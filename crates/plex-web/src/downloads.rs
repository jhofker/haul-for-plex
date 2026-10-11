//! In-memory download queue with live progress.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use plex_core::{Outcome, PlannedFile, ServerClient};
use tokio::sync::Semaphore;
use tokio::task::AbortHandle;

#[derive(Debug, Clone)]
pub enum JobState {
    Queued,
    Downloading {
        downloaded: u64,
        total: Option<u64>,
        /// Bytes already on disk when this attempt began (resumed download).
        base: u64,
        started: Instant,
    },
    Done {
        bytes: u64,
        resumed_from: u64,
    },
    Skipped,
    Failed(String),
}

impl JobState {
    pub fn is_active(&self) -> bool {
        matches!(self, JobState::Queued | JobState::Downloading { .. })
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: u64,
    pub server: String,
    pub dest: PathBuf,
    pub state: JobState,
}

#[derive(Clone)]
pub struct Downloads(Arc<Inner>);

struct Inner {
    jobs: Mutex<Vec<Job>>,
    handles: Mutex<HashMap<u64, AbortHandle>>,
    next_id: AtomicU64,
    slots: Semaphore,
}

impl Downloads {
    pub fn new(parallel: usize) -> Self {
        Downloads(Arc::new(Inner {
            jobs: Mutex::new(Vec::new()),
            handles: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            slots: Semaphore::new(parallel.max(1)),
        }))
    }

    /// Queue `files`; returns how many were added. Files already queued or
    /// downloading to the same path are not added twice.
    pub fn enqueue(&self, client: &ServerClient, server: &str, files: Vec<PlannedFile>) -> usize {
        let mut added = 0;
        for file in files {
            let id = {
                let mut jobs = self.0.jobs.lock().unwrap();
                if jobs
                    .iter()
                    .any(|j| j.dest == file.dest && j.state.is_active())
                {
                    continue;
                }
                let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
                jobs.push(Job {
                    id,
                    server: server.to_string(),
                    dest: file.dest.clone(),
                    state: JobState::Queued,
                });
                id
            };
            let this = self.clone();
            let client = client.clone();
            let task = tokio::spawn(async move { this.run(id, client, file).await });
            self.0
                .handles
                .lock()
                .unwrap()
                .insert(id, task.abort_handle());
            added += 1;
        }
        added
    }

    async fn run(&self, id: u64, client: ServerClient, file: PlannedFile) {
        // The permit is held for the whole download, bounding concurrency.
        let _slot = self.0.slots.acquire().await;
        let result = client
            .download_part(&file.part, &file.dest, false, |p| {
                self.update(id, |job| match &mut job.state {
                    JobState::Downloading {
                        downloaded, total, ..
                    } => {
                        *downloaded = p.downloaded;
                        *total = p.total;
                    }
                    state => {
                        *state = JobState::Downloading {
                            downloaded: p.downloaded,
                            total: p.total,
                            base: p.downloaded,
                            started: Instant::now(),
                        };
                    }
                });
            })
            .await;
        let state = match result {
            Ok(Outcome::Skipped) => JobState::Skipped,
            Ok(Outcome::Downloaded { resumed_from }) => JobState::Done {
                bytes: std::fs::metadata(&file.dest).map(|m| m.len()).unwrap_or(0),
                resumed_from,
            },
            Err(e) => JobState::Failed(e.to_string()),
        };
        self.update(id, |job| job.state = state);
        self.0.handles.lock().unwrap().remove(&id);
    }

    fn update(&self, id: u64, f: impl FnOnce(&mut Job)) {
        if let Some(job) = self.0.jobs.lock().unwrap().iter_mut().find(|j| j.id == id) {
            f(job);
        }
    }

    /// Stop a queued or running download. A partial file is kept so it can
    /// be resumed by downloading the item again.
    pub fn cancel(&self, id: u64) {
        if let Some(handle) = self.0.handles.lock().unwrap().remove(&id) {
            handle.abort();
        }
        self.update(id, |job| {
            if job.state.is_active() {
                job.state = JobState::Failed("cancelled".into());
            }
        });
    }

    /// Forget finished, skipped and failed jobs.
    pub fn clear_finished(&self) {
        self.0.jobs.lock().unwrap().retain(|j| j.state.is_active());
    }

    pub fn snapshot(&self) -> Vec<Job> {
        self.0.jobs.lock().unwrap().clone()
    }
}

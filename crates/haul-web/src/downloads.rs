//! The download queue: live progress, pause and resume, automatic retry when
//! a server drops, and a copy on disk so it survives a restart.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use haul_core::{Error, Outcome, Part, PlannedFile, Progress, Result, RetryPolicy, ServerClient};
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

/// Connects to a server by machine id. With `fresh` set, a cached connection
/// must not be reused: the server dropped and the best route may have changed.
pub type Connector =
    Arc<dyn Fn(String, bool) -> BoxFuture<'static, Result<ServerClient>> + Send + Sync>;

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
    /// The server dropped or can't be reached; tried again at `retry_at`.
    /// Only appears in [`Downloads::snapshot`]; stored jobs stay `Queued`.
    Waiting {
        reason: String,
        retry_at: Instant,
    },
    Paused,
    Done {
        bytes: u64,
        resumed_from: u64,
    },
    Skipped,
    Failed(String),
}

impl JobState {
    /// Not finished yet: still to run, running, waiting or paused.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            JobState::Queued
                | JobState::Downloading { .. }
                | JobState::Waiting { .. }
                | JobState::Paused
        )
    }

    /// Can be paused: queued, running or waiting for its server.
    pub fn can_pause(&self) -> bool {
        self.is_active() && !matches!(self, JobState::Paused)
    }

    /// Can be resumed (or, after a failure, retried).
    pub fn can_resume(&self) -> bool {
        matches!(self, JobState::Paused | JobState::Failed(_))
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: u64,
    /// Machine id of the server.
    pub server_id: String,
    /// Server name, for display.
    pub server: String,
    /// The item this file belongs to and the version chosen, for the record.
    pub item: String,
    pub version: String,
    pub part: Part,
    pub dest: PathBuf,
    pub state: JobState,
}

#[derive(Clone)]
pub struct Downloads(Arc<Inner>);

struct Inner {
    queue: Mutex<Queue>,
    parallel: usize,
    /// Backoff between attempts on a dropped server; `retries` is ignored,
    /// since the queue keeps waiting for the server to come back.
    retry: RetryPolicy,
    /// Where the queue is saved; `None` keeps it in memory only.
    path: Option<PathBuf>,
    /// Serialises saves so an older copy never overwrites a newer one.
    save_lock: Mutex<()>,
    connector: OnceLock<Connector>,
    stopped: AtomicBool,
}

struct Queue {
    jobs: Vec<Job>,
    running: HashMap<u64, Running>,
    /// Servers that dropped, by machine id.
    down: HashMap<String, Down>,
    next_id: u64,
}

struct Running {
    server_id: String,
    task: JoinHandle<()>,
}

struct Down {
    failures: u32,
    until: Instant,
    reason: String,
}

/// Waits between attempts on a dropped server: 5 seconds, doubling to 5 minutes.
pub const RETRY: RetryPolicy = RetryPolicy {
    retries: u32::MAX,
    first_delay: Duration::from_secs(5),
    max_delay: Duration::from_secs(300),
};

impl Downloads {
    /// A queue running `parallel` downloads at once, saved at `path` (and
    /// loaded from it, if it exists). Nothing runs until [`start`](Self::start).
    pub fn new(parallel: usize, path: Option<PathBuf>, retry: RetryPolicy) -> Self {
        let jobs = path.as_deref().map(load).unwrap_or_default();
        let next_id = jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
        Downloads(Arc::new(Inner {
            queue: Mutex::new(Queue {
                jobs,
                running: HashMap::new(),
                down: HashMap::new(),
                next_id,
            }),
            parallel: parallel.max(1),
            retry,
            path,
            save_lock: Mutex::new(()),
            connector: OnceLock::new(),
            stopped: AtomicBool::new(false),
        }))
    }

    /// Start downloading, using `connector` to reach servers. Saved jobs that
    /// were queued or running pick up from their partial files.
    pub fn start(&self, connector: Connector) {
        let _ = self.0.connector.set(connector);
        self.pump();
    }

    /// Queue `files` from server `server_id` (named `server`); returns how
    /// many were added. A file already in the queue isn't added twice; if
    /// it was paused, it is resumed.
    pub fn enqueue(
        &self,
        server_id: &str,
        server: &str,
        item: &str,
        version: &str,
        files: Vec<PlannedFile>,
    ) -> usize {
        let mut added = 0;
        {
            let mut q = self.0.queue.lock().unwrap();
            for file in files {
                if let Some(job) = q
                    .jobs
                    .iter_mut()
                    .find(|j| j.dest == file.dest && j.state.is_active())
                {
                    if matches!(job.state, JobState::Paused) {
                        job.state = JobState::Queued;
                    }
                    continue;
                }
                let id = q.next_id;
                q.next_id += 1;
                q.jobs.push(Job {
                    id,
                    server_id: server_id.to_string(),
                    server: server.to_string(),
                    item: item.to_string(),
                    version: version.to_string(),
                    part: file.part,
                    dest: file.dest,
                    state: JobState::Queued,
                });
                added += 1;
            }
        }
        self.changed();
        added
    }

    /// Pause a job: a running download stops, keeping its partial file, and
    /// its slot goes to the next job.
    pub fn pause(&self, id: u64) {
        self.pause_where(|j| j.id == id);
    }

    /// Pause every unfinished job, or only those from server `server_id`.
    pub fn pause_all(&self, server_id: Option<&str>) {
        self.pause_where(|j| server_id.is_none_or(|s| j.server_id == s));
    }

    fn pause_where(&self, matches: impl Fn(&Job) -> bool) {
        {
            let mut q = self.0.queue.lock().unwrap();
            let Queue { jobs, running, .. } = &mut *q;
            for job in jobs.iter_mut().filter(|j| matches(j)) {
                if job.state.can_pause() {
                    job.state = JobState::Paused;
                    if let Some(r) = running.remove(&job.id) {
                        r.task.abort();
                    }
                }
            }
        }
        self.changed();
    }

    /// Resume a paused job, or retry a failed one, from its partial file.
    pub fn resume(&self, id: u64) {
        self.resume_where(|j| j.id == id, true);
    }

    /// Resume every paused job, or only those from server `server_id`.
    /// Failed jobs are left alone.
    pub fn resume_all(&self, server_id: Option<&str>) {
        self.resume_where(|j| server_id.is_none_or(|s| j.server_id == s), false);
    }

    fn resume_where(&self, matches: impl Fn(&Job) -> bool, failed_too: bool) {
        {
            let mut q = self.0.queue.lock().unwrap();
            for job in q.jobs.iter_mut().filter(|j| matches(j)) {
                let resume = match job.state {
                    JobState::Paused => true,
                    JobState::Failed(_) => failed_too,
                    _ => false,
                };
                if resume {
                    job.state = JobState::Queued;
                }
            }
        }
        self.changed();
    }

    /// Stop a job for good. A partial file is kept so it can be resumed by
    /// downloading the item again.
    pub fn cancel(&self, id: u64) {
        {
            let mut q = self.0.queue.lock().unwrap();
            if let Some(r) = q.running.remove(&id) {
                r.task.abort();
            }
            if let Some(job) = q.jobs.iter_mut().find(|j| j.id == id)
                && job.state.is_active()
            {
                job.state = JobState::Failed("cancelled".into());
            }
        }
        self.changed();
    }

    /// Forget finished, skipped and failed jobs.
    pub fn clear_finished(&self) {
        self.0
            .queue
            .lock()
            .unwrap()
            .jobs
            .retain(|j| j.state.is_active());
        self.changed();
    }

    /// The jobs in queue order, with jobs of a dropped server shown as waiting.
    pub fn snapshot(&self) -> Vec<Job> {
        let q = self.0.queue.lock().unwrap();
        q.jobs
            .iter()
            .map(|job| {
                let mut job = job.clone();
                if matches!(job.state, JobState::Queued)
                    && let Some(down) = q.down.get(&job.server_id)
                {
                    job.state = JobState::Waiting {
                        reason: down.reason.clone(),
                        retry_at: down.until,
                    };
                }
                job
            })
            .collect()
    }

    /// Stop every running download, keeping partial files, and save the
    /// queue so those jobs carry on after a restart.
    pub async fn shutdown(&self) {
        self.0.stopped.store(true, Ordering::SeqCst);
        let tasks: Vec<_> = {
            let mut q = self.0.queue.lock().unwrap();
            q.running.drain().map(|(_, r)| r.task).collect()
        };
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        self.save();
    }

    /// Save, then start whatever can run.
    fn changed(&self) {
        self.save();
        self.pump();
    }

    /// Start queued jobs while there are free slots. A dropped server gets
    /// one job at a time, once its wait is over, to see if it is back.
    fn pump(&self) {
        if self.0.connector.get().is_none() || self.0.stopped.load(Ordering::SeqCst) {
            return;
        }
        let now = Instant::now();
        let mut q = self.0.queue.lock().unwrap();
        let mut free = self.0.parallel.saturating_sub(q.running.len());
        let mut i = 0;
        while free > 0 && i < q.jobs.len() {
            let job = &q.jobs[i];
            i += 1;
            if !matches!(job.state, JobState::Queued) || q.running.contains_key(&job.id) {
                continue;
            }
            let ready = match q.down.get(&job.server_id) {
                None => true,
                Some(down) => {
                    down.until <= now && !q.running.values().any(|r| r.server_id == job.server_id)
                }
            };
            if !ready {
                continue;
            }
            let fresh = q.down.contains_key(&job.server_id);
            let (id, server_id) = (job.id, job.server_id.clone());
            let file = PlannedFile {
                part: job.part.clone(),
                dest: job.dest.clone(),
            };
            let this = self.clone();
            let sid = server_id.clone();
            // The task can't finish before it is recorded: that needs this lock.
            let task = tokio::spawn(async move { this.run(id, sid, fresh, file).await });
            q.running.insert(id, Running { server_id, task });
            free -= 1;
        }
    }

    async fn run(self, id: u64, server_id: String, fresh: bool, file: PlannedFile) {
        let connector = self.0.connector.get().expect("started").clone();
        let result = match connector(server_id.clone(), fresh).await {
            Ok(client) => {
                client
                    .download_part(&file.part, &file.dest, false, |p| {
                        self.progress(id, &server_id, p)
                    })
                    .await
            }
            Err(e) => Err(e),
        };
        self.finish(id, &server_id, &file, result);
    }

    fn progress(&self, id: u64, server_id: &str, p: Progress) {
        let recovered = {
            let mut q = self.0.queue.lock().unwrap();
            let Some(job) = q.jobs.iter_mut().find(|j| j.id == id) else {
                return;
            };
            match &mut job.state {
                JobState::Downloading {
                    downloaded, total, ..
                } => {
                    *downloaded = p.downloaded;
                    *total = p.total;
                    false
                }
                state => {
                    *state = JobState::Downloading {
                        downloaded: p.downloaded,
                        total: p.total,
                        base: p.downloaded,
                        started: Instant::now(),
                    };
                    // The server is answering again: its other jobs can go.
                    q.down.remove(server_id).is_some()
                }
            }
        };
        if recovered {
            self.pump();
        }
    }

    fn finish(&self, id: u64, server_id: &str, file: &PlannedFile, result: Result<Outcome>) {
        let retry_in = {
            let mut q = self.0.queue.lock().unwrap();
            q.running.remove(&id);
            let mut retry_in = None;
            let state = match result {
                Ok(outcome) => {
                    q.down.remove(server_id);
                    match outcome {
                        Outcome::Skipped => JobState::Skipped,
                        Outcome::Downloaded { resumed_from } => JobState::Done {
                            bytes: std::fs::metadata(&file.dest).map(|m| m.len()).unwrap_or(0),
                            resumed_from,
                        },
                    }
                }
                Err(e) if e.is_retryable() => {
                    let now = Instant::now();
                    let down = q.down.entry(server_id.to_string()).or_insert(Down {
                        failures: 0,
                        until: now,
                        reason: String::new(),
                    });
                    // Several jobs failing together count as one failure.
                    if down.until <= now {
                        down.failures += 1;
                        down.until = now + self.0.retry.delay(down.failures);
                    }
                    down.reason = e.to_string();
                    retry_in = Some(down.until - now);
                    JobState::Queued
                }
                Err(e) => JobState::Failed(describe(&e)),
            };
            // A job paused or cancelled meanwhile keeps that state.
            if let Some(job) = q.jobs.iter_mut().find(|j| j.id == id)
                && matches!(job.state, JobState::Queued | JobState::Downloading { .. })
            {
                job.state = state;
            }
            retry_in
        };
        if let Some(wait) = retry_in {
            let this = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(wait).await;
                this.pump();
            });
        }
        self.changed();
    }

    fn save(&self) {
        let Some(path) = &self.0.path else { return };
        let _guard = self.0.save_lock.lock().unwrap();
        let saved = SavedQueue {
            jobs: self
                .0
                .queue
                .lock()
                .unwrap()
                .jobs
                .iter()
                .map(SavedJob::from)
                .collect(),
        };
        if let Err(e) = write_atomic(path, &saved) {
            tracing::warn!(
                "could not save the download queue to {}: {e}",
                path.display()
            );
        }
    }
}

/// A failure message for the queue, worded for someone looking at the page.
fn describe(e: &Error) -> String {
    match e {
        Error::ServerNotFound(_) => "this server is no longer shared with you".into(),
        Error::NotLoggedIn => "not signed in; sign in and retry".into(),
        e => e.to_string(),
    }
}

// ---- saved queue ----------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct SavedQueue {
    jobs: Vec<SavedJob>,
}

#[derive(Serialize, Deserialize)]
struct SavedJob {
    id: u64,
    server_id: String,
    server: String,
    item: String,
    version: String,
    dest: PathBuf,
    part: SavedPart,
    #[serde(flatten)]
    state: SavedState,
}

#[derive(Serialize, Deserialize)]
struct SavedPart {
    id: u64,
    key: String,
    size: Option<u64>,
    container: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum SavedState {
    Queued,
    Paused,
    Done { bytes: u64, resumed_from: u64 },
    Skipped,
    Failed { message: String },
}

impl From<&Job> for SavedJob {
    fn from(job: &Job) -> Self {
        let state = match &job.state {
            // Running or waiting jobs carry on after a restart.
            JobState::Queued | JobState::Downloading { .. } | JobState::Waiting { .. } => {
                SavedState::Queued
            }
            JobState::Paused => SavedState::Paused,
            JobState::Done {
                bytes,
                resumed_from,
            } => SavedState::Done {
                bytes: *bytes,
                resumed_from: *resumed_from,
            },
            JobState::Skipped => SavedState::Skipped,
            JobState::Failed(message) => SavedState::Failed {
                message: message.clone(),
            },
        };
        SavedJob {
            id: job.id,
            server_id: job.server_id.clone(),
            server: job.server.clone(),
            item: job.item.clone(),
            version: job.version.clone(),
            dest: job.dest.clone(),
            part: SavedPart {
                id: job.part.id,
                key: job.part.key.clone(),
                size: job.part.size,
                container: job.part.container.clone(),
            },
            state,
        }
    }
}

impl From<SavedJob> for Job {
    fn from(saved: SavedJob) -> Self {
        let state = match saved.state {
            SavedState::Queued => JobState::Queued,
            SavedState::Paused => JobState::Paused,
            SavedState::Done {
                bytes,
                resumed_from,
            } => JobState::Done {
                bytes,
                resumed_from,
            },
            SavedState::Skipped => JobState::Skipped,
            SavedState::Failed { message } => JobState::Failed(message),
        };
        Job {
            id: saved.id,
            server_id: saved.server_id,
            server: saved.server,
            item: saved.item,
            version: saved.version,
            part: Part {
                id: saved.part.id,
                key: saved.part.key,
                file: None,
                size: saved.part.size,
                container: saved.part.container,
                duration: None,
                streams: Vec::new(),
            },
            dest: saved.dest,
            state,
        }
    }
}

/// Jobs saved at `path`. A missing file is an empty queue; an unreadable one
/// is moved aside (so it isn't overwritten) and also starts an empty queue.
fn load(path: &Path) -> Vec<Job> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!("could not read the download queue {}: {e}", path.display());
            return Vec::new();
        }
    };
    match serde_json::from_slice::<SavedQueue>(&bytes) {
        Ok(saved) => saved.jobs.into_iter().map(Job::from).collect(),
        Err(e) => {
            let aside = path.with_extension("json.bad");
            tracing::warn!(
                "the download queue {} is unreadable ({e}); moved it to {} and started empty",
                path.display(),
                aside.display()
            );
            let _ = std::fs::rename(path, aside);
            Vec::new()
        }
    }
}

fn write_atomic(path: &Path, queue: &SavedQueue) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(queue).expect("queue serializes");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;

    use haul_core::{ClientInfo, Resource};
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const BODY: &[u8] = b"pretend this is a video file";

    fn quick_retry() -> RetryPolicy {
        RetryPolicy {
            retries: u32::MAX,
            first_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(40),
        }
    }

    /// A fake server named `id` whose parts `/p/<n>` serve [`BODY`].
    async fn server(id: &str) -> (MockServer, ServerClient) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/identity"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"MediaContainer": {"machineIdentifier": id}})),
            )
            .mount(&server)
            .await;
        let resource: Resource = serde_json::from_value(json!({
            "name": id, "clientIdentifier": id, "provides": "server", "accessToken": "t",
            "connections": [{"protocol": "http", "address": "127.0.0.1", "port": 1,
                             "uri": server.uri(), "local": true}],
        }))
        .unwrap();
        let client = ServerClient::connect(&resource, &ClientInfo::new("test"), None)
            .await
            .unwrap();
        (server, client)
    }

    async fn serve_part(server: &MockServer, n: u32, response: ResponseTemplate) {
        Mock::given(method("GET"))
            .and(path(format!("/p/{n}")))
            .respond_with(response)
            .with_priority(2)
            .mount(server)
            .await;
    }

    fn file(dir: &Path, n: u32) -> PlannedFile {
        PlannedFile {
            part: Part {
                id: n.into(),
                key: format!("/p/{n}"),
                file: None,
                size: Some(BODY.len() as u64),
                container: None,
                duration: None,
                streams: Vec::new(),
            },
            dest: dir.join(format!("{n}.mkv")),
        }
    }

    /// Connects to the given clients by id; counts fresh reconnects. An id
    /// with no client is no longer on the account.
    fn connector(clients: Vec<ServerClient>, fresh_count: Arc<AtomicU32>) -> Connector {
        Arc::new(move |id: String, fresh: bool| {
            if fresh {
                fresh_count.fetch_add(1, Ordering::SeqCst);
            }
            let client = clients.iter().find(|c| c.name() == id).cloned();
            Box::pin(async move { client.ok_or(Error::ServerNotFound(id)) })
        })
    }

    async fn wait_for(downloads: &Downloads, what: &str, ok: impl Fn(&[Job]) -> bool) {
        for _ in 0..200 {
            if ok(&downloads.snapshot()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("timed out waiting for {what}: {:#?}", downloads.snapshot());
    }

    fn state(jobs: &[Job], id: u64) -> &JobState {
        &jobs.iter().find(|j| j.id == id).unwrap().state
    }

    #[tokio::test]
    async fn queue_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let queue = dir.path().join("queue.json");
        let downloads = Downloads::new(1, Some(queue.clone()), quick_retry());
        let files = (1..=3).map(|n| file(dir.path(), n)).collect();
        assert_eq!(downloads.enqueue("srv", "Home", "10", "first", files), 3);
        downloads.pause(2);
        downloads.cancel(3);

        // A restart: the same file, read by a fresh queue.
        let downloads = Downloads::new(1, Some(queue.clone()), quick_retry());
        let jobs = downloads.snapshot();
        assert!(matches!(state(&jobs, 1), JobState::Queued));
        assert!(matches!(state(&jobs, 2), JobState::Paused));
        assert!(matches!(state(&jobs, 3), JobState::Failed(m) if m == "cancelled"));
        assert_eq!(jobs[0].server, "Home");
        assert_eq!(jobs[0].item, "10");
        assert_eq!(jobs[0].part.key, "/p/1");
        let more = vec![file(dir.path(), 4)];
        downloads.enqueue("srv", "Home", "11", "first", more);
        assert_eq!(downloads.snapshot().last().unwrap().id, 4);

        // Started, the queued job resumes from its partial file; the paused one waits.
        std::fs::write(dir.path().join("1.mkv.part"), &BODY[..5]).unwrap();
        let (pms, client) = server("srv").await;
        for n in 1..=4 {
            serve_part(&pms, n, ResponseTemplate::new(200).set_body_bytes(BODY)).await;
        }
        downloads.start(connector(vec![client], Default::default()));
        wait_for(&downloads, "jobs 1 and 4 done", |jobs| {
            matches!(state(jobs, 1), JobState::Done { .. })
                && matches!(state(jobs, 4), JobState::Done { .. })
        })
        .await;
        assert!(matches!(state(&downloads.snapshot(), 2), JobState::Paused));
        assert_eq!(std::fs::read(dir.path().join("1.mkv")).unwrap(), BODY);

        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&queue).unwrap()).unwrap();
        assert_eq!(saved["jobs"][0]["state"], "done");
        assert_eq!(saved["jobs"][1]["state"], "paused");
    }

    #[tokio::test]
    async fn unreadable_queue_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let queue = dir.path().join("queue.json");
        std::fs::write(&queue, b"not json").unwrap();
        let downloads = Downloads::new(1, Some(queue.clone()), quick_retry());
        assert!(downloads.snapshot().is_empty());
        assert!(dir.path().join("queue.json.bad").exists());
    }

    #[tokio::test]
    async fn server_errors_are_retried_with_a_fresh_connection() {
        let dir = tempfile::tempdir().unwrap();
        let (pms, client) = server("srv").await;
        Mock::given(method("GET"))
            .and(path("/p/1"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .with_priority(1)
            .mount(&pms)
            .await;
        serve_part(&pms, 1, ResponseTemplate::new(200).set_body_bytes(BODY)).await;

        let fresh = Arc::new(AtomicU32::new(0));
        let downloads = Downloads::new(1, None, quick_retry());
        downloads.start(connector(vec![client], fresh.clone()));
        downloads.enqueue("srv", "Home", "10", "first", vec![file(dir.path(), 1)]);
        wait_for(&downloads, "done", |jobs| {
            matches!(state(jobs, 1), JobState::Done { .. })
        })
        .await;
        assert_eq!(fresh.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_dropped_server_waits_while_other_servers_carry_on() {
        let dir = tempfile::tempdir().unwrap();
        let (down, down_client) = server("down").await;
        serve_part(&down, 1, ResponseTemplate::new(502)).await;
        let (up, up_client) = server("up").await;
        serve_part(&up, 2, ResponseTemplate::new(200).set_body_bytes(BODY)).await;

        let slow = RetryPolicy {
            first_delay: Duration::from_secs(60),
            max_delay: Duration::from_secs(60),
            ..quick_retry()
        };
        let downloads = Downloads::new(1, None, slow);
        downloads.start(connector(vec![down_client, up_client], Default::default()));
        downloads.enqueue("down", "Down", "1", "first", vec![file(dir.path(), 1)]);
        downloads.enqueue("up", "Up", "2", "first", vec![file(dir.path(), 2)]);

        wait_for(&downloads, "the other server's job", |jobs| {
            matches!(state(jobs, 2), JobState::Done { .. })
        })
        .await;
        let jobs = downloads.snapshot();
        match state(&jobs, 1) {
            JobState::Waiting { reason, retry_at } => {
                assert!(reason.contains("502"), "{reason}");
                assert!(*retry_at > Instant::now() + Duration::from_secs(30));
            }
            other => panic!("expected waiting, got {other:?}"),
        }
        // Only one attempt was made: the server isn't hammered while it's down.
        assert_eq!(
            down.received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|r| r.url.path() == "/p/1")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn permanent_errors_fail_without_retrying() {
        let dir = tempfile::tempdir().unwrap();
        let (pms, client) = server("srv").await;
        serve_part(&pms, 1, ResponseTemplate::new(404)).await;
        serve_part(&pms, 2, ResponseTemplate::new(403)).await;

        let fresh = Arc::new(AtomicU32::new(0));
        let downloads = Downloads::new(2, None, quick_retry());
        downloads.start(connector(vec![client], fresh.clone()));
        let files = vec![file(dir.path(), 1), file(dir.path(), 2)];
        downloads.enqueue("srv", "Home", "10", "first", files);
        wait_for(&downloads, "both failed", |jobs| {
            jobs.iter().all(|j| matches!(j.state, JobState::Failed(_)))
        })
        .await;
        let jobs = downloads.snapshot();
        assert!(matches!(state(&jobs, 1), JobState::Failed(m) if m.contains("404")));
        assert!(matches!(state(&jobs, 2), JobState::Failed(m) if m.contains("refused")));
        assert_eq!(fresh.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_server_no_longer_shared_fails_clearly() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = Downloads::new(1, None, quick_retry());
        downloads.start(connector(Vec::new(), Default::default()));
        downloads.enqueue("gone", "Gone", "10", "first", vec![file(dir.path(), 1)]);
        wait_for(
            &downloads,
            "failed",
            |jobs| matches!(state(jobs, 1), JobState::Failed(m) if m.contains("no longer shared")),
        )
        .await;
    }

    #[tokio::test]
    async fn pausing_frees_the_slot_and_resume_continues() {
        let dir = tempfile::tempdir().unwrap();
        let (pms, client) = server("srv").await;
        // Job 1 hangs, job 2 is quick.
        let stuck = ResponseTemplate::new(200)
            .set_body_bytes(BODY)
            .set_delay(Duration::from_secs(30));
        serve_part(&pms, 1, stuck).await;
        serve_part(&pms, 2, ResponseTemplate::new(200).set_body_bytes(BODY)).await;

        let downloads = Downloads::new(1, None, quick_retry());
        downloads.start(connector(vec![client], Default::default()));
        let files = vec![file(dir.path(), 1), file(dir.path(), 2)];
        downloads.enqueue("srv", "Home", "10", "first", files);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(matches!(state(&downloads.snapshot(), 2), JobState::Queued));

        downloads.pause(1);
        wait_for(&downloads, "job 2 done", |jobs| {
            matches!(state(jobs, 2), JobState::Done { .. })
        })
        .await;
        assert!(matches!(state(&downloads.snapshot(), 1), JobState::Paused));

        downloads.resume(1);
        assert!(matches!(state(&downloads.snapshot(), 1), JobState::Queued));
        assert_eq!(downloads.0.queue.lock().unwrap().running.len(), 1);
    }

    #[tokio::test]
    async fn pause_all_and_resume_all_can_target_one_server() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = Downloads::new(1, None, quick_retry());
        downloads.enqueue("a", "A", "1", "first", vec![file(dir.path(), 1)]);
        downloads.enqueue("b", "B", "2", "first", vec![file(dir.path(), 2)]);

        downloads.pause_all(Some("a"));
        let jobs = downloads.snapshot();
        assert!(matches!(state(&jobs, 1), JobState::Paused));
        assert!(matches!(state(&jobs, 2), JobState::Queued));

        downloads.pause_all(None);
        assert!(
            downloads
                .snapshot()
                .iter()
                .all(|j| matches!(j.state, JobState::Paused))
        );
        downloads.resume_all(Some("b"));
        let jobs = downloads.snapshot();
        assert!(matches!(state(&jobs, 1), JobState::Paused));
        assert!(matches!(state(&jobs, 2), JobState::Queued));

        // Queuing a paused file again resumes it.
        downloads.enqueue("a", "A", "1", "first", vec![file(dir.path(), 1)]);
        assert!(matches!(state(&downloads.snapshot(), 1), JobState::Queued));
    }

    #[tokio::test]
    async fn shutdown_stops_downloads_and_saves_them_as_queued() {
        let dir = tempfile::tempdir().unwrap();
        let queue = dir.path().join("queue.json");
        let (pms, client) = server("srv").await;
        let stuck = ResponseTemplate::new(200)
            .set_body_bytes(BODY)
            .set_delay(Duration::from_secs(30));
        serve_part(&pms, 1, stuck).await;

        let downloads = Downloads::new(1, Some(queue.clone()), quick_retry());
        downloads.start(connector(vec![client], Default::default()));
        downloads.enqueue("srv", "Home", "10", "first", vec![file(dir.path(), 1)]);
        tokio::time::sleep(Duration::from_millis(100)).await;
        downloads.shutdown().await;
        assert!(downloads.0.queue.lock().unwrap().running.is_empty());

        let restarted = Downloads::new(1, Some(queue), quick_retry());
        assert!(matches!(state(&restarted.snapshot(), 1), JobState::Queued));
    }
}

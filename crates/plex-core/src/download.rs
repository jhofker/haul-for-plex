//! Downloading media files from a server into the Plex-style layout.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, CONTENT_RANGE, RANGE};
use tokio::io::AsyncWriteExt;

use crate::error::{Error, Result};
use crate::library::{Metadata, Part};
use crate::naming::{self, Episode};
use crate::server::ServerClient;

/// Where movies and TV episodes are saved.
#[derive(Debug, Clone)]
pub struct Layout {
    pub movies_dir: PathBuf,
    pub tv_dir: PathBuf,
}

/// One file to fetch and where it will be saved.
#[derive(Debug, Clone)]
pub struct PlannedFile {
    pub part: Part,
    pub dest: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    /// Bytes on disk so far, including any resumed portion.
    pub downloaded: u64,
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Downloaded {
        /// Bytes that were already on disk from an interrupted attempt.
        resumed_from: u64,
    },
    /// The destination already exists with the expected size.
    Skipped,
}

impl ServerClient {
    /// Work out which files `item` consists of and where each should be saved.
    ///
    /// `item` must come from [`ServerClient::metadata`], which includes the
    /// media files. When an item has several versions (e.g. 1080p and 4K), the
    /// first is used.
    pub async fn plan_download(
        &self,
        item: &Metadata,
        layout: &Layout,
    ) -> Result<Vec<PlannedFile>> {
        if !item.is_playable() {
            return Err(Error::NotDownloadable {
                title: item.title.clone(),
                kind: item.kind.clone(),
            });
        }
        let media = item
            .media
            .first()
            .filter(|m| !m.parts.is_empty())
            .ok_or_else(|| Error::NoMedia(item.title.clone()))?;

        // An episode's own `year` is its air year; the folder uses the show's.
        let (root, show) = if item.kind == "episode" {
            let show_year = match &item.grandparent_rating_key {
                Some(key) => self.metadata(key).await?.year,
                None => None,
            };
            (&layout.tv_dir, Some(show_year))
        } else {
            (&layout.movies_dir, None)
        };

        let parts = media.parts.len();
        Ok(media
            .parts
            .iter()
            .enumerate()
            .map(|(i, part)| {
                let ext = part_extension(part, media.container.as_deref());
                let relative = match show {
                    Some(show_year) => naming::episode_path(
                        &Episode {
                            show: item.grandparent_title.as_deref().unwrap_or("Unknown Show"),
                            show_year,
                            season: item.parent_index,
                            number: item.index,
                            title: &item.title,
                        },
                        i + 1,
                        parts,
                        &ext,
                    ),
                    None => naming::movie_path(&item.title, item.year, i + 1, parts, &ext),
                };
                PlannedFile {
                    part: part.clone(),
                    dest: root.join(relative),
                }
            })
            .collect())
    }

    /// Download `part` to `dest`.
    ///
    /// Data goes to `<dest>.part` first and is renamed once complete, so an
    /// interrupted download never looks finished and is resumed (HTTP `Range`)
    /// the next time. An existing `dest` of the expected size is left alone;
    /// one of a different size is only replaced when `overwrite` is set.
    pub async fn download_part(
        &self,
        part: &Part,
        dest: &Path,
        overwrite: bool,
        mut on_progress: impl FnMut(Progress),
    ) -> Result<Outcome> {
        let expected = part.size;

        match tokio::fs::metadata(dest).await {
            Ok(meta) if expected == Some(meta.len()) => return Ok(Outcome::Skipped),
            Ok(_) if !overwrite => return Err(Error::DestinationExists(dest.to_path_buf())),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(dest, e)),
        }
        if let Some(dir) = dest.parent() {
            tokio::fs::create_dir_all(dir)
                .await
                .map_err(|e| Error::io(dir, e))?;
        }

        let tmp = partial_path(dest);
        let mut existing = file_len(&tmp).await?;
        if expected.is_some_and(|e| existing > e) {
            // Longer than the real file: it can't be a prefix of it.
            existing = 0;
        }

        let resumed_from = if existing > 0 && expected == Some(existing) {
            // Everything was downloaded but never renamed.
            existing
        } else {
            let (resp, start) = self.request_part(part, existing).await?;
            let status = resp.status();
            if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
                return Err(Error::DownloadForbidden(self.name().to_string()));
            }
            if !status.is_success() {
                let url = resp.url().to_string();
                let body = resp.text().await.unwrap_or_default();
                return Err(Error::Status { status, url, body });
            }

            // A server that ignores `Range` answers 200 with the whole file.
            let append = start > 0 && status == StatusCode::PARTIAL_CONTENT;
            let base = if append { start } else { 0 };
            let total = expected.or_else(|| resp.content_length().map(|l| l + base));

            let mut file = if append {
                tokio::fs::OpenOptions::new().append(true).open(&tmp).await
            } else {
                tokio::fs::File::create(&tmp).await
            }
            .map_err(|e| Error::io(&tmp, e))?;

            let mut downloaded = base;
            on_progress(Progress { downloaded, total });
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                // On a network error the partial file is kept for resuming.
                let chunk = chunk?;
                file.write_all(&chunk)
                    .await
                    .map_err(|e| Error::io(&tmp, e))?;
                downloaded += chunk.len() as u64;
                on_progress(Progress { downloaded, total });
            }
            file.flush().await.map_err(|e| Error::io(&tmp, e))?;
            file.sync_all().await.map_err(|e| Error::io(&tmp, e))?;
            base
        };

        let actual = file_len(&tmp).await?;
        if let Some(expected) = expected
            && actual != expected
        {
            if actual > expected {
                // Can't be resumed from; start over next time.
                let _ = tokio::fs::remove_file(&tmp).await;
            }
            return Err(Error::SizeMismatch { expected, actual });
        }

        tokio::fs::rename(&tmp, dest)
            .await
            .map_err(|e| Error::io(dest, e))?;
        Ok(Outcome::Downloaded { resumed_from })
    }

    /// Request `part` from byte `start` (0 for the whole file). Returns the
    /// response and the offset it actually starts at, which is 0 if the server
    /// rejected the range and a full copy was requested instead.
    async fn request_part(&self, part: &Part, start: u64) -> Result<(reqwest::Response, u64)> {
        let path = format!("{}?download=1", part.key.trim_start_matches('/'));
        let mut start = start;
        loop {
            let mut req = self.get(&path)?.header(ACCEPT, "*/*");
            if start > 0 {
                req = req.header(RANGE, format!("bytes={start}-"));
            }
            let resp = req.send().await?;
            let stale = start > 0
                && match resp.status() {
                    StatusCode::RANGE_NOT_SATISFIABLE => true,
                    StatusCode::PARTIAL_CONTENT => !content_range_starts_at(&resp, start),
                    _ => false,
                };
            if stale {
                start = 0;
                continue;
            }
            return Ok((resp, start));
        }
    }
}

fn content_range_starts_at(resp: &reqwest::Response, start: u64) -> bool {
    resp.headers()
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with(&format!("bytes {start}-")))
}

fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

async fn file_len(path: &Path) -> Result<u64> {
    match tokio::fs::metadata(path).await {
        Ok(meta) => Ok(meta.len()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(Error::io(path, e)),
    }
}

/// File extension for a part: its container, else the extension in its key.
fn part_extension(part: &Part, media_container: Option<&str>) -> String {
    part.container
        .as_deref()
        .or(media_container)
        .map(str::to_string)
        .or_else(|| {
            Path::new(&part.key)
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "bin".into())
}

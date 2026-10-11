//! Downloading media files from a server into the Plex-style layout.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, CONTENT_RANGE, RANGE};
use tokio::io::AsyncWriteExt;

use crate::error::{Error, Result};
use crate::library::{Media, Metadata, Part};
use crate::naming::{self, Episode};
use crate::server::ServerClient;

/// Where movies and TV episodes are saved.
#[derive(Debug, Clone)]
pub struct Layout {
    pub movies_dir: PathBuf,
    pub tv_dir: PathBuf,
}

/// Which version of an item to download when it has several (e.g. 1080p and 4K).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum VersionChoice {
    /// The first version the server lists.
    #[default]
    First,
    /// The Nth version (1-based), as numbered by `ls`.
    Index(usize),
    /// A resolution: `4k`, `1080`, `720`, `480`, `sd`.
    Resolution(String),
}

impl std::str::FromStr for VersionChoice {
    type Err = String;

    /// `first`, a number up to 99 (the Nth version), or a resolution such as
    /// `4k`, `2160`, `1080p`, `720`, `sd`.
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        let s = s.trim().to_ascii_lowercase();
        if s.is_empty() || s == "first" || s == "default" {
            return Ok(Self::First);
        }
        if s.len() <= 2 && s.chars().all(|c| c.is_ascii_digit()) {
            return match s.parse::<usize>() {
                Ok(n) if n >= 1 => Ok(Self::Index(n)),
                _ => Err("version numbers start at 1".into()),
            };
        }
        let res = match s.trim_end_matches('p') {
            "4k" | "2160" | "uhd" => "4k".to_string(),
            "sd" | "480" | "576" => s.trim_end_matches('p').to_string(),
            r if r.len() >= 3 && r.chars().all(|c| c.is_ascii_digit()) => r.to_string(),
            _ => {
                return Err(format!(
                    "{s:?} is not a version; use a number like 2, or a resolution like 4k, 1080p, 720p"
                ));
            }
        };
        Ok(Self::Resolution(res))
    }
}

impl std::fmt::Display for VersionChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::First => write!(f, "first"),
            Self::Index(n) => write!(f, "{n}"),
            Self::Resolution(r) => write!(f, "{r}"),
        }
    }
}

/// Normalises Plex's `videoResolution` for comparison with a `Resolution` choice.
fn normalize_resolution(r: &str) -> String {
    match r.to_ascii_lowercase().trim_end_matches('p') {
        "4k" | "2160" => "4k".into(),
        other => other.to_string(),
    }
}

impl VersionChoice {
    /// Index into `item.media` of the version to use. With `strict`, a choice
    /// the item can't satisfy is an error; otherwise (episodes of a season or
    /// show) the first version is used instead.
    fn pick(&self, item: &Metadata, strict: bool) -> Result<usize> {
        let usable = |m: &Media| !m.parts.is_empty();
        let first = item
            .media
            .iter()
            .position(usable)
            .ok_or_else(|| Error::NoMedia(item.title.clone()))?;
        let found = match self {
            Self::First => Some(first),
            Self::Index(n) => item.media.get(n - 1).filter(|m| usable(m)).map(|_| n - 1),
            Self::Resolution(r) => item.media.iter().position(|m| {
                usable(m)
                    && m.video_resolution
                        .as_deref()
                        .is_some_and(|v| normalize_resolution(v) == *r)
            }),
        };
        match found {
            Some(i) => Ok(i),
            None if !strict => Ok(first),
            None => Err(Error::NoSuchVersion {
                title: item.title.clone(),
                wanted: self.to_string(),
                available: item
                    .media
                    .iter()
                    .enumerate()
                    .map(|(i, m)| format!("{}) {}", i + 1, m.summary()))
                    .collect(),
            }),
        }
    }
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
    /// Movies and episodes give their own files; a season or show expands to
    /// all of its episodes, in order. `item` must come from
    /// [`ServerClient::metadata`], which includes the media files. When an item
    /// has several versions (e.g. 1080p and 4K), the first is used; see
    /// [`plan_download_version`](Self::plan_download_version) to choose.
    pub async fn plan_download(
        &self,
        item: &Metadata,
        layout: &Layout,
    ) -> Result<Vec<PlannedFile>> {
        self.plan_download_version(item, layout, &VersionChoice::First)
            .await
    }

    /// Like [`plan_download`](Self::plan_download), choosing among versions.
    ///
    /// A version other than the first is saved with its resolution in the
    /// file name (`Movie (2023) - 4K.mkv`), so versions can sit side by side.
    /// For a movie or episode a version that doesn't exist is an error; for
    /// a season or show, episodes lacking it use their first version.
    pub async fn plan_download_version(
        &self,
        item: &Metadata,
        layout: &Layout,
        version: &VersionChoice,
    ) -> Result<Vec<PlannedFile>> {
        match item.kind.as_str() {
            "movie" => plan_files(item, &layout.movies_dir, None, version, true),
            "episode" => {
                let show_year = self
                    .show_year(item.grandparent_rating_key.as_deref())
                    .await?;
                plan_files(item, &layout.tv_dir, Some(show_year), version, true)
            }
            "show" | "season" => {
                let (episodes, show_year) = if item.kind == "show" {
                    (self.all_leaves(&item.rating_key).await?, item.year)
                } else {
                    (
                        self.children(&item.rating_key).await?,
                        self.show_year(item.parent_rating_key.as_deref()).await?,
                    )
                };
                let mut files = Vec::new();
                for episode in episodes.iter().filter(|e| e.kind == "episode") {
                    // An episode without files can't be downloaded; skip it
                    // rather than failing the whole season.
                    if episode.media.iter().any(|m| !m.parts.is_empty()) {
                        files.extend(plan_files(
                            episode,
                            &layout.tv_dir,
                            Some(show_year),
                            version,
                            false,
                        )?);
                    }
                }
                if files.is_empty() {
                    return Err(Error::NoMedia(item.title.clone()));
                }
                Ok(files)
            }
            _ => Err(Error::NotDownloadable {
                title: item.title.clone(),
                kind: item.kind.clone(),
            }),
        }
    }

    /// The year a show started, from its record.
    async fn show_year(&self, show_key: Option<&str>) -> Result<Option<u32>> {
        match show_key {
            Some(key) => Ok(self.metadata(key).await?.year),
            None => Ok(None),
        }
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

/// Name for a non-default version, used in the file name: `4K`, `1080p`, or
/// `Version 2` when the server gives no resolution (or two share one).
fn edition_name(item: &Metadata, index: usize) -> String {
    let media = &item.media[index];
    let label = media.resolution_label();
    let shared = label.is_some()
        && item
            .media
            .iter()
            .enumerate()
            .any(|(i, m)| i != index && m.resolution_label() == label);
    match label {
        Some(l) if !shared => l,
        Some(l) => format!("{l} v{}", index + 1),
        None => format!("Version {}", index + 1),
    }
}

/// Files for one movie or episode. `show_year` is `Some` for episodes.
fn plan_files(
    item: &Metadata,
    root: &Path,
    show_year: Option<Option<u32>>,
    version: &VersionChoice,
    strict: bool,
) -> Result<Vec<PlannedFile>> {
    let index = version.pick(item, strict)?;
    let media = &item.media[index];
    // The first usable version keeps the plain name Plex expects.
    let first = item.media.iter().position(|m| !m.parts.is_empty());
    let edition = (Some(index) != first).then(|| edition_name(item, index));
    let edition = edition.as_deref();
    let parts = media.parts.len();
    Ok(media
        .parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            let ext = part_extension(part, media.container.as_deref());
            let relative = match show_year {
                Some(show_year) => naming::episode_path(
                    &Episode {
                        show: item.grandparent_title.as_deref().unwrap_or("Unknown Show"),
                        show_year,
                        season: item.parent_index,
                        number: item.index,
                        title: &item.title,
                        edition,
                    },
                    i + 1,
                    parts,
                    &ext,
                ),
                None => naming::movie_path(&item.title, item.year, edition, i + 1, parts, &ext),
            };
            PlannedFile {
                part: part.clone(),
                dest: root.join(relative),
            }
        })
        .collect())
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

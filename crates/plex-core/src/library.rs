//! Browsing a server: libraries, their items, and item details.

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::plextv::send;
use crate::server::ServerClient;

/// Items fetched per request when listing a whole library.
const PAGE_SIZE: usize = 500;

/// A library section, e.g. "Movies" or "TV Shows".
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    pub key: String,
    pub title: String,
    /// `movie`, `show`, `artist` or `photo`.
    #[serde(rename = "type")]
    pub kind: String,
}

/// A movie, show, season or episode.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub rating_key: String,
    /// `movie`, `show`, `season`, `episode`, ...
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub year: Option<u32>,
    /// Season number for seasons, episode number for episodes.
    #[serde(default)]
    pub index: Option<u32>,
    /// Season number, for episodes.
    #[serde(default)]
    pub parent_index: Option<u32>,
    /// Show title, for seasons.
    #[serde(default)]
    pub parent_title: Option<String>,
    /// Show title, for episodes.
    #[serde(default)]
    pub grandparent_title: Option<String>,
    /// Show year is not on episodes; `parentYear`/`grandparentYear` are rarely set.
    #[serde(default)]
    pub parent_year: Option<u32>,
    #[serde(default)]
    pub grandparent_year: Option<u32>,
    #[serde(default)]
    pub parent_rating_key: Option<String>,
    #[serde(default)]
    pub grandparent_rating_key: Option<String>,
    /// Name of the library the item is in; set on search results.
    #[serde(default)]
    pub library_section_title: Option<String>,
    /// Key of the library the item is in.
    #[serde(default, rename = "librarySectionID")]
    pub library_section_id: Option<u64>,
    /// Number of episodes under a show or season.
    #[serde(default)]
    pub leaf_count: Option<u32>,
    /// Number of seasons under a show.
    #[serde(default)]
    pub child_count: Option<u32>,
    #[serde(default, rename = "Media")]
    pub media: Vec<Media>,
}

impl Metadata {
    /// True for items that have files: movies and episodes.
    pub fn is_playable(&self) -> bool {
        matches!(self.kind.as_str(), "movie" | "episode")
    }

    /// Total size of the first media version's files, if known.
    pub fn size(&self) -> Option<u64> {
        let media = self.media.first()?;
        media.parts.iter().map(|p| p.size).sum()
    }
}

/// One version of an item (e.g. 1080p vs 4K); usually only one.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    #[serde(default)]
    pub container: Option<String>,
    #[serde(default)]
    pub video_resolution: Option<String>,
    #[serde(default)]
    pub video_codec: Option<String>,
    #[serde(default, rename = "Part")]
    pub parts: Vec<Part>,
}

/// One file of a media version; multi-part movies have several.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub id: u64,
    /// Server path for fetching the file, e.g. `/library/parts/1/123/file.mkv`.
    pub key: String,
    /// Path of the file on the server's disk.
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub container: Option<String>,
}

#[derive(Deserialize)]
struct Envelope<T> {
    #[serde(rename = "MediaContainer")]
    container: T,
}

#[derive(Deserialize)]
struct Directories {
    #[serde(default, rename = "Directory")]
    directories: Vec<Library>,
}

#[derive(Deserialize)]
struct Hubs {
    #[serde(default, rename = "Hub")]
    hubs: Vec<Hub>,
}

#[derive(Deserialize)]
struct Hub {
    #[serde(default, rename = "Metadata")]
    metadata: Vec<Metadata>,
}

/// Item kinds search returns; music, photos and people are left out since
/// only video can be downloaded into the movie/TV layout.
const SEARCHABLE_KINDS: [&str; 4] = ["movie", "show", "season", "episode"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MetadataList {
    #[serde(default, rename = "Metadata")]
    metadata: Vec<Metadata>,
    #[serde(default)]
    total_size: Option<usize>,
}

impl ServerClient {
    pub async fn libraries(&self) -> Result<Vec<Library>> {
        let env: Envelope<Directories> = send(self.get("library/sections")?).await?;
        Ok(env.container.directories)
    }

    /// One page of a library's top-level items, plus the library's total size.
    pub async fn library_page(
        &self,
        library_key: &str,
        start: usize,
        size: usize,
    ) -> Result<(Vec<Metadata>, usize)> {
        let req = self
            .get(&format!("library/sections/{library_key}/all"))?
            .header("X-Plex-Container-Start", start.to_string())
            .header("X-Plex-Container-Size", size.to_string());
        let env: Envelope<MetadataList> = send(req).await?;
        let page = env.container.metadata;
        let total = env.container.total_size.unwrap_or(start + page.len());
        Ok((page, total))
    }

    /// Every top-level item in a library (movies, or shows), fetched in pages.
    pub async fn library_items(&self, library_key: &str) -> Result<Vec<Metadata>> {
        let path = format!("library/sections/{library_key}/all");
        let mut items = Vec::new();
        loop {
            let req = self
                .get(&path)?
                .header("X-Plex-Container-Start", items.len().to_string())
                .header("X-Plex-Container-Size", PAGE_SIZE.to_string());
            let env: Envelope<MetadataList> = send(req).await?;
            let page = env.container.metadata;
            let fetched = page.len();
            items.extend(page);
            let done = match env.container.total_size {
                Some(total) => items.len() >= total,
                None => fetched < PAGE_SIZE,
            };
            if done || fetched == 0 {
                return Ok(items);
            }
        }
    }

    /// Search every library on the server for movies, shows, seasons and episodes.
    /// `limit` caps results per kind.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<Metadata>> {
        let params = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("query", query)
            .append_pair("limit", &limit.to_string())
            .finish();
        let env: Envelope<Hubs> = send(self.get(&format!("hubs/search?{params}"))?).await?;
        Ok(env
            .container
            .hubs
            .into_iter()
            .flat_map(|hub| hub.metadata)
            .filter(|m| SEARCHABLE_KINDS.contains(&m.kind.as_str()))
            .collect())
    }

    /// Full details of one item, including its media files.
    pub async fn metadata(&self, rating_key: &str) -> Result<Metadata> {
        let env: Envelope<MetadataList> =
            match send(self.get(&format!("library/metadata/{rating_key}"))?).await {
                Err(Error::Status { status, .. }) if status == reqwest::StatusCode::NOT_FOUND => {
                    return Err(Error::ItemNotFound(rating_key.into()));
                }
                other => other?,
            };
        env.container
            .metadata
            .into_iter()
            .next()
            .ok_or_else(|| Error::ItemNotFound(rating_key.into()))
    }

    /// Seasons of a show, or episodes of a season.
    pub async fn children(&self, rating_key: &str) -> Result<Vec<Metadata>> {
        let env: Envelope<MetadataList> =
            send(self.get(&format!("library/metadata/{rating_key}/children"))?).await?;
        Ok(env.container.metadata)
    }

    /// Every episode of a show (or season), across seasons.
    pub async fn all_leaves(&self, rating_key: &str) -> Result<Vec<Metadata>> {
        let env: Envelope<MetadataList> =
            send(self.get(&format!("library/metadata/{rating_key}/allLeaves"))?).await?;
        Ok(env.container.metadata)
    }
}

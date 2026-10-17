//! Browsing a server: libraries, their items, and item details.

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::listing::View;
use crate::plextv::send;
use crate::server::ServerClient;

/// Items fetched per request when listing a whole library.
const PAGE_SIZE: usize = 500;

/// Asks listings to include outside ids (`Guid`), used to match items
/// between servers.
const INCLUDE_GUIDS: (&str, &str) = ("includeGuids", "1");

/// Plex's number for the episode type, for listing a library's episodes.
const EPISODE_TYPE: u8 = 4;

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
    /// Plex's own id for the item, e.g. `plex://movie/5d77...`; older agents
    /// use forms like `com.plexapp.agents.imdb://tt0111161?lang=en`.
    #[serde(default)]
    pub guid: Option<String>,
    /// Ids from outside databases, e.g. `imdb://tt0111161` or `tmdb://278`.
    /// Listings only include them when asked (`includeGuids=1`).
    #[serde(default, rename = "Guid")]
    pub guids: Vec<Guid>,
    /// The season's `guid`, for episodes; the show's, for seasons.
    #[serde(default)]
    pub parent_guid: Option<String>,
    /// The show's `guid`, for episodes.
    #[serde(default)]
    pub grandparent_guid: Option<String>,
    /// Title to sort by, e.g. `Matrix` for `The Matrix`; only sent when it differs.
    #[serde(default)]
    pub title_sort: Option<String>,
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
    /// Plot summary.
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub tagline: Option<String>,
    /// Runtime in milliseconds.
    #[serde(default)]
    pub duration: Option<u64>,
    /// Age rating such as `PG-13`.
    #[serde(default)]
    pub content_rating: Option<String>,
    /// Critic rating, 0-10.
    #[serde(default)]
    pub rating: Option<f64>,
    /// Audience rating, 0-10.
    #[serde(default)]
    pub audience_rating: Option<f64>,
    #[serde(default)]
    pub studio: Option<String>,
    /// Release / air date, `YYYY-MM-DD`.
    #[serde(default)]
    pub originally_available_at: Option<String>,
    /// When the item was added to the library (unix seconds).
    #[serde(default)]
    pub added_at: Option<i64>,
    #[serde(default, rename = "Genre")]
    pub genres: Vec<Tag>,
    #[serde(default, rename = "Director")]
    pub directors: Vec<Tag>,
    #[serde(default, rename = "Writer")]
    pub writers: Vec<Tag>,
    #[serde(default, rename = "Country")]
    pub countries: Vec<Tag>,
    #[serde(default, rename = "Role")]
    pub roles: Vec<Tag>,
    #[serde(default, rename = "Media")]
    pub media: Vec<Media>,
}

/// A genre, person or country attached to an item.
#[derive(Debug, Clone, Deserialize)]
pub struct Tag {
    pub tag: String,
    /// Character name, on cast entries.
    #[serde(default)]
    pub role: Option<String>,
}

/// An outside id for an item, e.g. `imdb://tt0111161`.
#[derive(Debug, Clone, Deserialize)]
pub struct Guid {
    pub id: String,
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
    #[serde(default)]
    pub video_profile: Option<String>,
    #[serde(default)]
    pub video_frame_rate: Option<String>,
    #[serde(default)]
    pub audio_codec: Option<String>,
    #[serde(default)]
    pub audio_profile: Option<String>,
    #[serde(default)]
    pub audio_channels: Option<u32>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub aspect_ratio: Option<f64>,
    /// Overall bitrate in kbps.
    #[serde(default)]
    pub bitrate: Option<u64>,
    /// Runtime in milliseconds.
    #[serde(default)]
    pub duration: Option<u64>,
    #[serde(default, rename = "Part")]
    pub parts: Vec<Part>,
}

impl Media {
    /// Resolution as users write it: `4K`, `1080p`, `720p`, `SD`, or the raw value.
    pub fn resolution_label(&self) -> Option<String> {
        let r = self.video_resolution.as_deref()?;
        Some(match r.to_ascii_lowercase().as_str() {
            "4k" => "4K".to_string(),
            "sd" => "SD".to_string(),
            d if d.chars().all(|c| c.is_ascii_digit()) => format!("{d}p"),
            other => other.to_string(),
        })
    }

    /// Short description for choosing between versions, e.g. `1080p h264 mkv`.
    pub fn summary(&self) -> String {
        let mut bits = Vec::new();
        bits.extend(self.resolution_label());
        bits.extend(self.video_codec.clone());
        bits.extend(self.container.clone());
        if bits.is_empty() {
            "unknown".into()
        } else {
            bits.join(" ")
        }
    }
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
    #[serde(default)]
    pub duration: Option<u64>,
    /// Video, audio and subtitle streams in the file.
    #[serde(default, rename = "Stream")]
    pub streams: Vec<Stream>,
}

/// One video, audio or subtitle stream inside a file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    /// 1 = video, 2 = audio, 3 = subtitle.
    pub stream_type: u8,
    #[serde(default)]
    pub codec: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    /// Plex's own summary, e.g. `English (DTS-HD MA 5.1)`.
    #[serde(default)]
    pub display_title: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    /// kbps.
    #[serde(default)]
    pub bitrate: Option<u64>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub frame_rate: Option<f64>,
    #[serde(default)]
    pub bit_depth: Option<u32>,
    #[serde(default)]
    pub channels: Option<u32>,
    #[serde(default)]
    pub color_space: Option<String>,
    /// Set on subtitle streams that are external files or forced.
    #[serde(default)]
    pub forced: Option<bool>,
    #[serde(default)]
    pub default: Option<bool>,
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

/// Lowercase letters and digits only, so "Spider-Man" and "spiderman" compare equal.
pub(crate) fn compact(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub(crate) fn tokens(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(compact)
        .filter(|t| !t.is_empty())
        .collect()
}

/// Plex's search is fuzzy and also matches summaries and cast; keep only items
/// whose title (or show/season title for episodes) contains every query word.
pub(crate) fn matches_title(m: &Metadata, tokens: &[String]) -> bool {
    let mut haystack = compact(&m.title);
    for extra in [&m.parent_title, &m.grandparent_title]
        .into_iter()
        .flatten()
    {
        haystack.push_str(&compact(extra));
    }
    tokens.iter().all(|t| haystack.contains(t.as_str()))
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

    /// One page of a library's top-level items, filtered and sorted by the
    /// server, plus how many items match in total.
    pub async fn library_page(
        &self,
        library_key: &str,
        start: usize,
        size: usize,
        view: &View,
    ) -> Result<(Vec<Metadata>, usize)> {
        let req = self
            .get(&format!("library/sections/{library_key}/all"))?
            .query(&view.plex_params())
            .query(&[INCLUDE_GUIDS])
            .header("X-Plex-Container-Start", start.to_string())
            .header("X-Plex-Container-Size", size.to_string());
        let env: Envelope<MetadataList> = send(req).await?;
        let page = env.container.metadata;
        let total = env.container.total_size.unwrap_or(start + page.len());
        Ok((page, total))
    }

    /// Every top-level item in a library (movies, or shows), filtered and
    /// sorted by the server, fetched in pages.
    pub async fn library_items(&self, library_key: &str, view: &View) -> Result<Vec<Metadata>> {
        let mut params = view.plex_params();
        params.push((INCLUDE_GUIDS.0, INCLUDE_GUIDS.1.to_string()));
        self.fetch_all(&format!("library/sections/{library_key}/all"), &params)
            .await
    }

    /// Every episode in a TV library, across all shows, fetched in pages.
    pub async fn library_episodes(&self, library_key: &str) -> Result<Vec<Metadata>> {
        let params = [("type", EPISODE_TYPE.to_string())];
        self.fetch_all(&format!("library/sections/{library_key}/all"), &params)
            .await
    }

    async fn fetch_all(&self, path: &str, params: &[(&str, String)]) -> Result<Vec<Metadata>> {
        let mut items = Vec::new();
        loop {
            let req = self
                .get(path)?
                .query(params)
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
        self.search_in(query, limit, None).await
    }

    /// Like [`search`](Self::search), optionally restricted to one library (by key).
    pub async fn search_in(
        &self,
        query: &str,
        limit: usize,
        library_key: Option<&str>,
    ) -> Result<Vec<Metadata>> {
        // Built in its own block so the serializer isn't held across the await.
        let params = {
            let mut p = url::form_urlencoded::Serializer::new(String::new());
            p.append_pair("query", query)
                .append_pair("limit", &limit.to_string())
                .append_pair(INCLUDE_GUIDS.0, INCLUDE_GUIDS.1);
            if let Some(key) = library_key {
                p.append_pair("sectionId", key);
            }
            p.finish()
        };
        let env: Envelope<Hubs> = send(self.get(&format!("hubs/search?{params}"))?).await?;
        let tokens = tokens(query);
        let mut seen = std::collections::HashSet::new();
        let mut found: Vec<Metadata> = env
            .container
            .hubs
            .into_iter()
            .flat_map(|hub| hub.metadata)
            .filter(|m| SEARCHABLE_KINDS.contains(&m.kind.as_str()))
            .filter(|m| seen.insert(m.rating_key.clone()))
            .filter(|m| matches_title(m, &tokens))
            // Servers don't reliably honour `sectionId`, so filter here as well.
            .filter(|m| match (library_key, m.library_section_id) {
                (Some(key), Some(id)) => key == id.to_string(),
                _ => true,
            })
            .collect();
        // Exact title first, then titles starting with the query, then the rest.
        let whole = compact(query);
        found.sort_by_key(|m| {
            let t = compact(&m.title);
            if t == whole {
                0
            } else if t.starts_with(&whole) {
                1
            } else {
                2
            }
        });
        Ok(found)
    }

    /// Cover art for an item, scaled down by the server to fit `width`×`height`.
    /// Returns the image bytes and their content type.
    pub async fn thumbnail(
        &self,
        rating_key: &str,
        width: u32,
        height: u32,
    ) -> Result<(Vec<u8>, String)> {
        let source = format!("/library/metadata/{rating_key}/thumb");
        let params = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("url", &source)
            .append_pair("width", &width.to_string())
            .append_pair("height", &height.to_string())
            .append_pair("minSize", "1")
            .finish();
        // Fall back to the original image if the server can't transcode.
        let mut last = None;
        for path in [
            format!("photo/:/transcode?{params}"),
            source[1..].to_string(),
        ] {
            let resp = self.get(&path)?.send().await?;
            let status = resp.status();
            if status.is_success() {
                let kind = resp
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("image/jpeg")
                    .to_string();
                return Ok((resp.bytes().await?.to_vec(), kind));
            }
            last = Some(Error::Status {
                status,
                url: resp.url().to_string(),
                body: String::new(),
            });
        }
        Err(last.expect("loop ran"))
    }

    /// Full details of one item, including its media files.
    pub async fn metadata(&self, rating_key: &str) -> Result<Metadata> {
        let env: Envelope<MetadataList> = match send(
            self.get(&format!("library/metadata/{rating_key}"))?
                .query(&[INCLUDE_GUIDS]),
        )
        .await
        {
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
        let env: Envelope<MetadataList> = send(
            self.get(&format!("library/metadata/{rating_key}/children"))?
                .query(&[INCLUDE_GUIDS]),
        )
        .await?;
        Ok(env.container.metadata)
    }

    /// Every episode of a show (or season), across seasons.
    pub async fn all_leaves(&self, rating_key: &str) -> Result<Vec<Metadata>> {
        let env: Envelope<MetadataList> = send(
            self.get(&format!("library/metadata/{rating_key}/allLeaves"))?
                .query(&[INCLUDE_GUIDS]),
        )
        .await?;
        Ok(env.container.metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str) -> Metadata {
        serde_json::from_value(serde_json::json!({
            "ratingKey": "1", "type": "movie", "title": title
        }))
        .unwrap()
    }

    #[test]
    fn title_match_ignores_punctuation_and_requires_all_words() {
        let t = tokens("spider man");
        assert!(matches_title(&item("Spider-Man: Homecoming"), &t));
        assert!(!matches_title(&item("Suits"), &tokens("hackers")));
        assert!(matches_title(&item("Hackers"), &tokens("HACKERS")));
        assert!(!matches_title(&item("Spider-Man"), &tokens("spider verse")));
    }
}

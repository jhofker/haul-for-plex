//! Comparing other servers with your own library: for each movie, show,
//! season and episode, whether you already have it, a better copy is on
//! offer, or it is missing.
//!
//! Items are matched by outside ids (`imdb://`, `tmdb://`, `tvdb://`) and
//! Plex's own `plex://` ids, falling back to title and year.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::library::{Media, Metadata, compact};
use crate::plextv::Resource;
use crate::server::ServerClient;

/// How an item on another server compares with your library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// On your server at the same or better quality.
    Have,
    /// On your server, but this copy has a higher resolution. Heights in
    /// pixels, e.g. 1080 and 2160.
    Upgrade { mine: u32, theirs: u32 },
    /// A show or season you have, missing about this many episodes.
    Partial { missing: u32 },
    /// Not on your server.
    Missing,
}

impl Status {
    /// Short machine name: `have`, `upgrade`, `partial` or `missing`.
    pub fn name(self) -> &'static str {
        match self {
            Status::Have => "have",
            Status::Upgrade { .. } => "upgrade",
            Status::Partial { .. } => "partial",
            Status::Missing => "missing",
        }
    }

    /// What to show a person, e.g. `Upgrade: 1080p → 4K`.
    pub fn label(self) -> String {
        match self {
            Status::Have => "Have it".into(),
            Status::Upgrade { mine, theirs } => {
                format!("Upgrade: {} → {}", height_label(mine), height_label(theirs))
            }
            Status::Partial { missing: 1 } => "1 episode missing".into(),
            Status::Partial { missing } => format!("{missing} episodes missing"),
            Status::Missing => "Missing".into(),
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

/// A video height as people say it: `4K`, `1080p`, `720p` and so on.
pub fn height_label(h: u32) -> String {
    if h >= 2000 {
        "4K".into()
    } else {
        format!("{h}p")
    }
}

/// Which statuses a list keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFilter {
    /// Anything with something to get: missing items, and shows or seasons
    /// with missing episodes.
    Missing,
    /// Better copies of things you have.
    Upgrade,
    /// Things you already have.
    Have,
}

impl StatusFilter {
    pub const ALL: [StatusFilter; 3] = [
        StatusFilter::Missing,
        StatusFilter::Upgrade,
        StatusFilter::Have,
    ];

    pub fn name(self) -> &'static str {
        match self {
            StatusFilter::Missing => "missing",
            StatusFilter::Upgrade => "upgrade",
            StatusFilter::Have => "have",
        }
    }

    /// Whether an item with `status` stays in the list. Items that can't be
    /// compared (`None`) never do.
    pub fn matches(self, status: Option<Status>) -> bool {
        matches!(
            (self, status),
            (
                StatusFilter::Missing,
                Some(Status::Missing | Status::Partial { .. })
            ) | (StatusFilter::Upgrade, Some(Status::Upgrade { .. }))
                | (StatusFilter::Have, Some(Status::Have))
        )
    }
}

impl FromStr for StatusFilter {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        let s = s.trim().to_ascii_lowercase();
        StatusFilter::ALL
            .into_iter()
            .find(|f| f.name() == s)
            .ok_or_else(|| format!("unknown status {s:?}; use missing, upgrade or have"))
    }
}

impl fmt::Display for StatusFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Whether `server` counts as your library: it is one of those chosen in
/// [`Config::my_servers`], or, when none are chosen, you own it.
pub fn is_mine(server: &Resource, config: &Config) -> bool {
    if config.my_servers.is_empty() {
        server.owned
    } else {
        config.my_servers.contains(&server.client_identifier)
    }
}

/// The servers that count as your library; see [`is_mine`].
pub fn my_servers<'a>(servers: &'a [Resource], config: &Config) -> Vec<&'a Resource> {
    servers.iter().filter(|s| is_mine(s, config)).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OwnedMovie {
    title: String,
    year: Option<u32>,
    guids: Vec<String>,
    /// Height of the best version, when known.
    best: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OwnedShow {
    title: String,
    year: Option<u32>,
    guids: Vec<String>,
    /// `[season, episode, height]` for each episode; height 0 when unknown.
    episodes: Vec<[u32; 3]>,
}

/// Lookup tables, rebuilt after loading.
#[derive(Debug, Clone, Default)]
struct Index {
    movie_guids: HashMap<String, usize>,
    movie_titles: HashMap<(String, Option<u32>), usize>,
    show_guids: HashMap<String, usize>,
    show_titles: HashMap<String, Vec<usize>>,
    /// Per show: (season, episode) → height (0 when unknown).
    episodes: Vec<HashMap<(u32, u32), u32>>,
}

/// What's on your own servers, for comparing other servers against.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MyLibrary {
    /// Machine ids of the servers compared against.
    pub servers: Vec<String>,
    /// Names of servers that could not be read.
    pub skipped: Vec<String>,
    /// When it was built, in unix seconds.
    pub built_at: u64,
    movies: Vec<OwnedMovie>,
    shows: Vec<OwnedShow>,
    #[serde(skip)]
    index: Index,
}

impl MyLibrary {
    /// Read the movie and TV libraries of `servers`. A server that can't be
    /// read is listed in [`skipped`](Self::skipped) instead of failing.
    pub async fn load(servers: &[(&Resource, &ServerClient)]) -> MyLibrary {
        let mut lib = MyLibrary {
            built_at: now(),
            ..MyLibrary::default()
        };
        let reads =
            futures_util::future::join_all(servers.iter().map(|(_, client)| read(client))).await;
        for ((resource, _), result) in servers.iter().zip(reads) {
            match result {
                Ok((movies, shows)) => {
                    lib.servers.push(resource.client_identifier.clone());
                    lib.movies.extend(movies);
                    lib.shows.extend(shows);
                }
                Err(e) => {
                    tracing::warn!("could not read your library on {}: {e}", resource.name);
                    lib.skipped.push(resource.name.clone());
                }
            }
        }
        lib.reindex();
        lib
    }

    /// How many movies and shows were found.
    pub fn counts(&self) -> (usize, usize) {
        (self.movies.len(), self.shows.len())
    }

    /// How long ago it was built.
    pub fn age(&self) -> Duration {
        Duration::from_secs(now().saturating_sub(self.built_at))
    }

    fn reindex(&mut self) {
        let mut index = Index::default();
        for (i, m) in self.movies.iter().enumerate() {
            for g in &m.guids {
                index.movie_guids.entry(g.clone()).or_insert(i);
            }
            index
                .movie_titles
                .entry((compact(&m.title), m.year))
                .or_insert(i);
        }
        for (i, s) in self.shows.iter().enumerate() {
            for g in &s.guids {
                index.show_guids.entry(g.clone()).or_insert(i);
            }
            index
                .show_titles
                .entry(compact(&s.title))
                .or_default()
                .push(i);
            index.episodes.push(
                s.episodes
                    .iter()
                    .map(|[season, ep, h]| ((*season, *ep), *h))
                    .collect(),
            );
        }
        self.index = index;
    }

    fn find_movie(&self, m: &Metadata) -> Option<&OwnedMovie> {
        let by_guid = item_guids(m).find_map(|g| self.index.movie_guids.get(&g));
        let i = by_guid.or_else(|| {
            let title = compact(&m.title);
            self.index
                .movie_titles
                .get(&(title.clone(), m.year))
                .or_else(|| {
                    // Without a year on one side, accept a title that is unique.
                    let mut hits = self
                        .index
                        .movie_titles
                        .iter()
                        .filter(|((t, y), _)| *t == title && (m.year.is_none() || y.is_none()));
                    match (hits.next(), hits.next()) {
                        (Some((_, i)), None) => Some(i),
                        _ => None,
                    }
                })
        })?;
        self.movies.get(*i)
    }

    /// Find a show by its ids, then by title (and year, when both sides know it).
    fn find_show<'a>(
        &self,
        guids: impl IntoIterator<Item = &'a str>,
        title: Option<&str>,
        year: Option<u32>,
    ) -> Option<usize> {
        let mut guids = guids.into_iter().filter_map(normalize_guid);
        if let Some(i) = guids.find_map(|g| self.index.show_guids.get(&g)) {
            return Some(*i);
        }
        let candidates = self.index.show_titles.get(&compact(title?))?;
        let fits: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|i| match (year, self.shows[*i].year) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            })
            .collect();
        match fits.as_slice() {
            [one] => Some(*one),
            _ => None,
        }
    }

    /// The show `item` is, or belongs to.
    fn show_of(&self, item: &Metadata) -> Option<usize> {
        match item.kind.as_str() {
            "show" => self.find_show(own_guids(item), Some(&item.title), item.year),
            "season" => self.find_show(
                item.parent_guid.as_deref(),
                item.parent_title.as_deref(),
                item.parent_year,
            ),
            "episode" => self.find_show(
                item.grandparent_guid.as_deref(),
                item.grandparent_title.as_deref(),
                item.grandparent_year,
            ),
            _ => None,
        }
    }

    /// How `item` compares with your library; `None` for things that can't
    /// be compared, such as music.
    pub fn status(&self, item: &Metadata) -> Option<Status> {
        if item.kind == "movie" {
            return Some(match self.find_movie(item) {
                None => Status::Missing,
                Some(owned) => quality(owned.best, best_height(&item.media)),
            });
        }
        if !matches!(item.kind.as_str(), "show" | "season" | "episode") {
            return None;
        }
        Some(self.status_in_show(self.show_of(item), item))
    }

    /// Like [`status`](Self::status) for a season or episode of `show`, which
    /// is matched instead of relying on the item's own show fields.
    pub fn status_under(&self, show: &Metadata, item: &Metadata) -> Option<Status> {
        if !matches!(item.kind.as_str(), "season" | "episode") {
            return self.status(item);
        }
        Some(self.status_in_show(self.show_of(show), item))
    }

    fn status_in_show(&self, show: Option<usize>, item: &Metadata) -> Status {
        let Some(show) = show else {
            return Status::Missing;
        };
        let episodes = &self.index.episodes[show];
        let have = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        match item.kind.as_str() {
            "show" => counted(have(episodes.len()), item.leaf_count),
            "season" => {
                let Some(season) = item.index else {
                    return Status::Have;
                };
                let n = episodes.keys().filter(|(s, _)| *s == season).count();
                counted(have(n), item.leaf_count)
            }
            _ => match (item.parent_index, item.index) {
                (Some(s), Some(e)) => match episodes.get(&(s, e)) {
                    None => Status::Missing,
                    Some(h) => quality((*h > 0).then_some(*h), best_height(&item.media)),
                },
                // Can't place it in the show; assume it's there.
                _ => Status::Have,
            },
        }
    }

    /// A test for the movies and episodes under `item` (a movie, episode,
    /// season or show) that are missing from your library, for downloading
    /// just those.
    pub fn missing_under<'a>(&'a self, item: &'a Metadata) -> impl Fn(&Metadata) -> bool + 'a {
        move |m| {
            let status = if item.kind == "show" {
                self.status_under(item, m)
            } else {
                self.status(m)
            };
            status == Some(Status::Missing)
        }
    }
}

/// Count-based status for a show or season: we know how many episodes the
/// other server has, not which.
fn counted(have: u32, theirs: Option<u32>) -> Status {
    if have == 0 {
        return Status::Missing;
    }
    match theirs {
        Some(n) if n > have => Status::Partial { missing: n - have },
        _ => Status::Have,
    }
}

fn quality(mine: Option<u32>, theirs: Option<u32>) -> Status {
    match (mine, theirs) {
        (Some(mine), Some(theirs)) if theirs > mine => Status::Upgrade { mine, theirs },
        _ => Status::Have,
    }
}

/// Height of a version in pixels, from Plex's resolution label or the
/// video height.
fn height(m: &Media) -> Option<u32> {
    let from_label = m.video_resolution.as_deref().and_then(|r| {
        match r.to_ascii_lowercase().trim_end_matches('p') {
            "4k" | "uhd" => Some(2160),
            "sd" => Some(480),
            d => d.parse().ok(),
        }
    });
    from_label.or(m.height)
}

fn best_height(media: &[Media]) -> Option<u32> {
    media.iter().filter_map(height).max()
}

/// Lower-cased ids in a form both new and old Plex agents agree on:
/// `plex://movie/…`, `imdb://tt…`, `tmdb://…`, `tvdb://…`. Local ids
/// (`local://`, `com.plexapp.agents.none`) say nothing and are dropped.
pub fn normalize_guid(g: &str) -> Option<String> {
    let g = g.trim().to_ascii_lowercase();
    if let Some(rest) = g.strip_prefix("com.plexapp.agents.") {
        let (agent, id) = rest.split_once("://")?;
        let id = id.split(['?', '/']).next().filter(|id| !id.is_empty())?;
        let scheme = match agent {
            "imdb" => "imdb",
            "themoviedb" | "tmdb" => "tmdb",
            "thetvdb" | "tvdb" => "tvdb",
            _ => return None,
        };
        return Some(format!("{scheme}://{id}"));
    }
    let (scheme, id) = g.split_once("://")?;
    if id.is_empty() || matches!(scheme, "local" | "file") {
        return None;
    }
    Some(g)
}

/// The item's own `guid` and its outside ids, as given.
fn own_guids(m: &Metadata) -> impl Iterator<Item = &str> {
    m.guid
        .as_deref()
        .into_iter()
        .chain(m.guids.iter().map(|g| g.id.as_str()))
}

fn item_guids(m: &Metadata) -> impl Iterator<Item = String> + '_ {
    own_guids(m).filter_map(normalize_guid)
}

fn guid_list(m: &Metadata) -> Vec<String> {
    let mut v: Vec<String> = item_guids(m).collect();
    v.dedup();
    v
}

/// One server's movies and shows.
async fn read(client: &ServerClient) -> Result<(Vec<OwnedMovie>, Vec<OwnedShow>)> {
    let mut movies = Vec::new();
    let mut shows = Vec::new();
    let all = crate::listing::View::default();
    for lib in client.libraries().await? {
        match lib.kind.as_str() {
            "movie" => {
                for m in client.library_items(&lib.key, &all).await? {
                    movies.push(OwnedMovie {
                        guids: guid_list(&m),
                        best: best_height(&m.media),
                        title: m.title,
                        year: m.year,
                    });
                }
            }
            "show" => {
                let mut by_key: BTreeMap<String, usize> = BTreeMap::new();
                for s in client.library_items(&lib.key, &all).await? {
                    by_key.insert(s.rating_key.clone(), shows.len());
                    shows.push(OwnedShow {
                        guids: guid_list(&s),
                        title: s.title,
                        year: s.year,
                        episodes: Vec::new(),
                    });
                }
                for e in client.library_episodes(&lib.key).await? {
                    let show = e
                        .grandparent_rating_key
                        .as_ref()
                        .and_then(|k| by_key.get(k));
                    if let (Some(show), Some(season), Some(ep)) = (show, e.parent_index, e.index) {
                        let h = best_height(&e.media).unwrap_or(0);
                        shows[*show].episodes.push([season, ep, h]);
                    }
                }
            }
            _ => {}
        }
    }
    Ok((movies, shows))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// [`MyLibrary`] saved to disk between runs, so the CLI needn't re-read a
/// big library every time.
#[derive(Debug, Clone)]
pub struct MyLibraryCache {
    path: PathBuf,
}

impl MyLibraryCache {
    /// Kept as `my-library.json` in `dir` (the config directory).
    pub fn in_dir(dir: impl AsRef<Path>) -> Self {
        MyLibraryCache {
            path: dir.as_ref().join("my-library.json"),
        }
    }

    /// The saved library, if it is younger than `max_age` and covers
    /// exactly `servers` (machine ids).
    pub fn load(&self, servers: &[&str], max_age: Duration) -> Option<MyLibrary> {
        let bytes = fs::read(&self.path).ok()?;
        let mut lib: MyLibrary = serde_json::from_slice(&bytes).ok()?;
        let mut want: Vec<&str> = servers.to_vec();
        let mut have: Vec<&str> = lib.servers.iter().map(String::as_str).collect();
        want.sort_unstable();
        have.sort_unstable();
        if want != have || !lib.skipped.is_empty() || lib.age() > max_age {
            return None;
        }
        lib.reindex();
        Some(lib)
    }

    pub fn save(&self, lib: &MyLibrary) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        let json = serde_json::to_vec(lib).expect("library serializes");
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, json).map_err(|e| Error::io(&tmp, e))?;
        fs::rename(&tmp, &self.path).map_err(|e| Error::io(&self.path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta(v: serde_json::Value) -> Metadata {
        serde_json::from_value(v).unwrap()
    }

    fn library() -> MyLibrary {
        let mut lib = MyLibrary {
            movies: vec![
                OwnedMovie {
                    title: "The Lantern Keeper".into(),
                    year: Some(2012),
                    guids: vec!["imdb://tt100".into()],
                    best: Some(1080),
                },
                OwnedMovie {
                    title: "Orbit Seven".into(),
                    year: Some(2021),
                    guids: vec![],
                    best: Some(2160),
                },
            ],
            shows: vec![OwnedShow {
                title: "Harbor Lights".into(),
                year: Some(2019),
                guids: vec!["tvdb://55".into()],
                episodes: vec![[1, 1, 1080], [1, 2, 720], [2, 1, 1080]],
            }],
            ..MyLibrary::default()
        };
        lib.reindex();
        lib
    }

    fn movie(title: &str, year: u32, guids: &[&str], res: &str) -> Metadata {
        let guids: Vec<_> = guids.iter().map(|g| json!({"id": g})).collect();
        meta(
            json!({"ratingKey": "1", "type": "movie", "title": title, "year": year,
                    "Guid": guids, "Media": [{"videoResolution": res}]}),
        )
    }

    #[test]
    fn guids_from_old_and_new_agents_agree() {
        assert_eq!(
            normalize_guid("com.plexapp.agents.imdb://tt0111161?lang=en").as_deref(),
            Some("imdb://tt0111161")
        );
        assert_eq!(
            normalize_guid("com.plexapp.agents.thetvdb://12345/1/2?lang=en").as_deref(),
            Some("tvdb://12345")
        );
        assert_eq!(
            normalize_guid("com.plexapp.agents.themoviedb://278?lang=en").as_deref(),
            Some("tmdb://278")
        );
        assert_eq!(
            normalize_guid("plex://movie/5D77").as_deref(),
            Some("plex://movie/5d77")
        );
        assert_eq!(normalize_guid("local://123"), None);
        assert_eq!(normalize_guid("com.plexapp.agents.none://abc"), None);
    }

    #[test]
    fn movies_match_by_id_then_title_and_year() {
        let lib = library();
        // Different title, same IMDb id.
        let m = movie("Lantern Keeper", 2012, &["imdb://tt100"], "1080");
        assert_eq!(lib.status(&m), Some(Status::Have));
        // A better copy.
        let m = movie("The Lantern Keeper", 2012, &[], "4k");
        assert_eq!(
            lib.status(&m),
            Some(Status::Upgrade {
                mine: 1080,
                theirs: 2160
            })
        );
        // A worse copy still counts as having it.
        let m = movie("Orbit Seven", 2021, &[], "720");
        assert_eq!(lib.status(&m), Some(Status::Have));
        // Same title, different year: a different film.
        let m = movie("Orbit Seven", 1990, &[], "720");
        assert_eq!(lib.status(&m), Some(Status::Missing));
        assert_eq!(
            lib.status(&movie("Nope", 2000, &[], "1080")),
            Some(Status::Missing)
        );
    }

    #[test]
    fn shows_seasons_and_episodes() {
        let lib = library();
        let show = meta(
            json!({"ratingKey": "9", "type": "show", "title": "Harbour Lights",
                               "guid": "com.plexapp.agents.thetvdb://55?lang=en", "leafCount": 5}),
        );
        assert_eq!(lib.status(&show), Some(Status::Partial { missing: 2 }));

        let season = |n: u32, eps: u32| {
            meta(
                json!({"ratingKey": "10", "type": "season", "title": "Season", "index": n,
                        "leafCount": eps, "parentTitle": "Harbor Lights"}),
            )
        };
        assert_eq!(lib.status(&season(1, 2)), Some(Status::Have));
        assert_eq!(
            lib.status(&season(2, 3)),
            Some(Status::Partial { missing: 2 })
        );
        assert_eq!(lib.status(&season(3, 3)), Some(Status::Missing));

        let ep = |s: u32, e: u32, res: &str| {
            meta(
                json!({"ratingKey": "11", "type": "episode", "title": "Ep", "index": e,
                        "parentIndex": s, "grandparentTitle": "Harbor Lights",
                        "Media": [{"videoResolution": res}]}),
            )
        };
        assert_eq!(lib.status(&ep(1, 1, "1080")), Some(Status::Have));
        assert_eq!(
            lib.status(&ep(1, 2, "1080")),
            Some(Status::Upgrade {
                mine: 720,
                theirs: 1080
            })
        );
        assert_eq!(lib.status(&ep(2, 2, "1080")), Some(Status::Missing));

        // Under a show matched by id, the episode's own show fields don't matter.
        let stray = meta(json!({"ratingKey": "12", "type": "episode", "title": "Ep",
                                "index": 1, "parentIndex": 2}));
        assert_eq!(lib.status_under(&show, &stray), Some(Status::Have));
        let missing = lib.missing_under(&show);
        assert!(!missing(&stray));
        assert!(missing(&ep(3, 1, "1080")));

        let unknown = meta(json!({"ratingKey": "13", "type": "show", "title": "Other"}));
        assert_eq!(lib.status(&unknown), Some(Status::Missing));
        let music = meta(json!({"ratingKey": "14", "type": "artist", "title": "Band"}));
        assert_eq!(lib.status(&music), None);
    }

    #[test]
    fn filters() {
        let f: StatusFilter = "Missing".parse().unwrap();
        assert!(f.matches(Some(Status::Missing)));
        assert!(f.matches(Some(Status::Partial { missing: 1 })));
        assert!(!f.matches(Some(Status::Have)));
        assert!(!f.matches(None));
        assert!("better".parse::<StatusFilter>().is_err());
        assert_eq!(
            Status::Upgrade {
                mine: 720,
                theirs: 2160
            }
            .label(),
            "Upgrade: 720p → 4K"
        );
    }

    #[test]
    fn cache_round_trips_and_expires() {
        let dir = tempfile::tempdir().unwrap();
        let cache = MyLibraryCache::in_dir(dir.path());
        let mut lib = library();
        lib.servers = vec!["a".into()];
        lib.built_at = now();
        cache.save(&lib).unwrap();

        let loaded = cache.load(&["a"], Duration::from_secs(60)).unwrap();
        assert_eq!(
            loaded.status(&movie("Orbit Seven", 2021, &[], "1080")),
            Some(Status::Have)
        );
        assert!(cache.load(&["a", "b"], Duration::from_secs(60)).is_none());
        lib.built_at = now() - 120;
        cache.save(&lib).unwrap();
        assert!(cache.load(&["a"], Duration::from_secs(60)).is_none());
    }
}

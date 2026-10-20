//! Shared application state: settings, cached server connections, login in progress.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use haul_core::plextv::Pin;
use haul_core::{
    ClientInfo, Config, ConfigStore, Error, Layout, MyLibrary, MyLibraryCache, PlexTvClient,
    Resource, Result, ServerClient,
};

use crate::downloads::Downloads;

/// How long the list of servers from plex.tv is reused.
const SERVER_LIST_TTL: Duration = Duration::from_secs(60);
/// How long a connected server client is reused before probing again.
const CLIENT_TTL: Duration = Duration::from_secs(300);
/// How long your own library is used before it is read again (in the
/// background; pages keep using the old copy meanwhile).
const MY_LIBRARY_TTL: Duration = Duration::from_secs(15 * 60);

/// Your own library, as far as a page comparing against it is concerned.
#[derive(Clone)]
pub enum Mine {
    /// Nothing to compare: the server is one of yours, or no server counts
    /// as your library.
    Off,
    /// Still being read for the first time.
    Loading,
    Ready(Arc<MyLibrary>),
}

impl Mine {
    pub fn ready(&self) -> Option<&MyLibrary> {
        match self {
            Mine::Ready(lib) => Some(lib),
            _ => None,
        }
    }
}

#[derive(Default)]
struct MineCache {
    lib: Option<Arc<MyLibrary>>,
    refreshing: bool,
}

#[derive(Debug, Clone)]
pub struct Settings {
    /// Where the Plex token and client id live; shared with the CLI.
    pub store: ConfigStore,
    pub layout: Layout,
    /// When set, the whole interface requires this password (HTTP basic auth).
    pub password: Option<String>,
    /// Files downloaded at the same time.
    pub parallel: usize,
    /// Alternative plex.tv address, for tests.
    pub plextv_url: Option<String>,
}

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    settings: Settings,
    downloads: Downloads,
    pending_pin: Mutex<Option<Pin>>,
    servers: Mutex<Option<(Instant, Vec<Resource>)>>,
    clients: Mutex<HashMap<String, (Instant, ServerClient)>>,
    mine: Mutex<MineCache>,
}

impl AppState {
    pub fn new(settings: Settings) -> Self {
        let downloads = Downloads::new(settings.parallel);
        AppState(Arc::new(Inner {
            settings,
            downloads,
            pending_pin: Mutex::new(None),
            servers: Mutex::new(None),
            clients: Mutex::new(HashMap::new()),
            mine: Mutex::new(MineCache::default()),
        }))
    }

    pub fn settings(&self) -> &Settings {
        &self.0.settings
    }

    pub fn downloads(&self) -> &Downloads {
        &self.0.downloads
    }

    /// The config file, re-read each time so CLI changes (e.g. logout) apply.
    pub fn config(&self) -> Result<Config> {
        self.0.settings.store.load_or_init()
    }

    /// Username of the signed-in account, if any.
    pub fn username(&self) -> Option<String> {
        self.config().ok()?.auth.map(|a| a.username)
    }

    fn client_info(config: &Config) -> ClientInfo {
        ClientInfo::new(&config.client_id)
    }

    fn plextv_for(&self, config: &Config) -> Result<PlexTvClient> {
        let info = Self::client_info(config);
        match &self.0.settings.plextv_url {
            Some(url) => PlexTvClient::with_base_url(info, url),
            None => PlexTvClient::new(info),
        }
    }

    /// plex.tv client without a token, for starting a login.
    pub fn plextv_anonymous(&self) -> Result<PlexTvClient> {
        self.plextv_for(&self.config()?)
    }

    /// plex.tv client using the saved token.
    pub fn plextv(&self) -> Result<PlexTvClient> {
        let config = self.config()?;
        Ok(self.plextv_for(&config)?.with_token(config.token()?))
    }

    pub fn set_pending_pin(&self, pin: Option<Pin>) {
        *self.0.pending_pin.lock().unwrap() = pin;
    }

    pub fn pending_pin(&self) -> Option<Pin> {
        self.0.pending_pin.lock().unwrap().clone()
    }

    /// Forget cached servers and connections (after login or logout).
    pub fn invalidate(&self) {
        *self.0.servers.lock().unwrap() = None;
        self.0.clients.lock().unwrap().clear();
        self.0.mine.lock().unwrap().lib = None;
    }

    /// Servers on the account, reusing a recent answer from plex.tv.
    pub async fn servers(&self) -> Result<Vec<Resource>> {
        if let Some((at, servers)) = &*self.0.servers.lock().unwrap()
            && at.elapsed() < SERVER_LIST_TTL
        {
            return Ok(servers.clone());
        }
        let servers = self.plextv()?.servers().await?;
        *self.0.servers.lock().unwrap() = Some((Instant::now(), servers.clone()));
        Ok(servers)
    }

    /// A connected client for the server with machine id `id`.
    pub async fn server(&self, id: &str) -> Result<(Resource, ServerClient)> {
        let servers = self.servers().await?;
        let resource = servers
            .into_iter()
            .find(|s| s.client_identifier == id)
            .ok_or_else(|| Error::ServerNotFound(id.to_string()))?;

        if let Some((at, client)) = self.0.clients.lock().unwrap().get(id)
            && at.elapsed() < CLIENT_TTL
        {
            return Ok((resource, client.clone()));
        }

        let mut config = self.config()?;
        let cached = config.connections.get(id).cloned();
        let client =
            ServerClient::connect(&resource, &Self::client_info(&config), cached.as_deref())
                .await?;

        let uri = client.reachable().connection.uri.clone();
        if cached.as_deref() != Some(uri.as_str()) {
            config.connections.insert(id.to_string(), uri);
            // Losing the connection hint only costs a probe next time.
            let _ = self.0.settings.store.save(&config);
        }
        self.0
            .clients
            .lock()
            .unwrap()
            .insert(id.to_string(), (Instant::now(), client.clone()));
        Ok((resource, client))
    }

    /// Whether `server` counts as your own library.
    pub fn is_mine(&self, server: &Resource) -> bool {
        self.config()
            .map(|c| haul_core::is_mine(server, &c))
            .unwrap_or(false)
    }

    /// Your library for comparing `server` against. Never waits: when it
    /// hasn't been read, or is getting old, it is read in the background.
    pub async fn mine_for(&self, server: &Resource) -> Mine {
        if self.is_mine(server) {
            return Mine::Off;
        }
        self.mine().await
    }

    /// Your library, or [`Mine::Loading`] while it is first read.
    pub async fn mine(&self) -> Mine {
        let Ok(ids) = self.my_server_ids().await else {
            return Mine::Off;
        };
        if ids.is_empty() {
            return Mine::Off;
        }
        let (lib, start) = {
            let mut cache = self.0.mine.lock().unwrap();
            // A saved copy from an earlier run (or the CLI) gets things going.
            if cache.lib.is_none() {
                let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                cache.lib = self.disk_cache().load(&refs, MY_LIBRARY_TTL).map(Arc::new);
            }
            let stale = cache.lib.as_ref().is_none_or(|l| {
                l.age() > MY_LIBRARY_TTL || !same_servers(&l.servers, &l.skipped, &ids)
            });
            let start = stale && !cache.refreshing;
            if start {
                cache.refreshing = true;
            }
            (cache.lib.clone(), start)
        };
        if start {
            let state = self.clone();
            tokio::spawn(async move {
                let lib = state.read_my_library().await;
                let mut cache = state.0.mine.lock().unwrap();
                cache.refreshing = false;
                match lib {
                    Ok(lib) => cache.lib = Some(lib),
                    Err(e) => tracing::warn!("could not read your library: {e}"),
                }
            });
        }
        match lib {
            Some(lib) => Mine::Ready(lib),
            None => Mine::Loading,
        }
    }

    /// Your library, reading it now if there's no copy yet (for actions
    /// such as downloading what's missing, which can't do without it).
    pub async fn mine_now(&self) -> Result<Option<Arc<MyLibrary>>> {
        match self.mine().await {
            Mine::Off => Ok(None),
            Mine::Ready(lib) => Ok(Some(lib)),
            Mine::Loading => {
                let lib = self.read_my_library().await?;
                self.0.mine.lock().unwrap().lib = Some(lib.clone());
                Ok(Some(lib))
            }
        }
    }

    /// Read your library again in the background, e.g. after choosing
    /// different servers.
    pub fn refresh_mine(&self) {
        self.0.mine.lock().unwrap().lib = None;
    }

    /// Whether your library is being read right now.
    pub fn mine_refreshing(&self) -> bool {
        self.0.mine.lock().unwrap().refreshing
    }

    async fn my_server_ids(&self) -> Result<Vec<String>> {
        let config = self.config()?;
        let servers = self.servers().await?;
        Ok(haul_core::my_servers(&servers, &config)
            .into_iter()
            .map(|s| s.client_identifier.clone())
            .collect())
    }

    fn disk_cache(&self) -> MyLibraryCache {
        let path = self.0.settings.store.path();
        MyLibraryCache::in_dir(path.parent().unwrap_or(".".as_ref()))
    }

    async fn read_my_library(&self) -> Result<Arc<MyLibrary>> {
        let config = self.config()?;
        let servers = self.servers().await?;
        let mine: Vec<Resource> = haul_core::my_servers(&servers, &config)
            .into_iter()
            .cloned()
            .collect();
        let connected = join_all(mine.iter().map(|s| self.server(&s.client_identifier))).await;
        let mut clients = Vec::new();
        let mut unreachable = Vec::new();
        for (resource, result) in mine.iter().zip(connected) {
            match result {
                Ok((_, client)) => clients.push((resource, client)),
                Err(_) => unreachable.push(resource.name.clone()),
            }
        }
        let pairs: Vec<_> = clients.iter().map(|(r, c)| (*r, c)).collect();
        let mut lib = MyLibrary::load(&pairs).await;
        lib.skipped.extend(unreachable);
        if lib.skipped.is_empty() {
            let _ = self.disk_cache().save(&lib);
        }
        Ok(Arc::new(lib))
    }
}

/// Whether a library that read the servers `read` and couldn't read
/// `skipped` (names) was built for exactly the servers `want`. Skipped
/// servers are retried when the copy goes stale, not on every page.
fn same_servers(read: &[String], skipped: &[String], want: &[String]) -> bool {
    read.iter().all(|id| want.contains(id)) && read.len() + skipped.len() == want.len()
}

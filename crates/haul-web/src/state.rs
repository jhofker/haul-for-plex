//! Shared application state: settings, cached server connections, login in progress.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use haul_core::plextv::Pin;
use haul_core::{
    ClientInfo, Config, ConfigStore, Error, Layout, PlexTvClient, Resource, Result, ServerClient,
};

use crate::downloads::Downloads;

/// How long the list of servers from plex.tv is reused.
const SERVER_LIST_TTL: Duration = Duration::from_secs(60);
/// How long a connected server client is reused before probing again.
const CLIENT_TTL: Duration = Duration::from_secs(300);

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
}

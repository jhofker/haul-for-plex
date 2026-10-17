//! Shared setup for commands that talk to Plex: config, plex.tv client, server connections.

use std::time::Duration;

use anyhow::{Result, bail};
use futures_util::future::join_all;
use haul_core::{
    ClientInfo, Config, ConfigStore, MyLibrary, MyLibraryCache, PlexTvClient, Resource,
    ServerClient,
};

/// How long a saved copy of your library is trusted before it is read again.
const MY_LIBRARY_MAX_AGE: Duration = Duration::from_secs(10 * 60);

pub struct Context {
    pub store: ConfigStore,
    pub config: Config,
    pub info: ClientInfo,
    servers: Option<Vec<Resource>>,
}

impl Context {
    pub fn load(store: ConfigStore) -> Result<Self> {
        let config = store.load_or_init()?;
        let info = ClientInfo::new(&config.client_id);
        Ok(Context {
            store,
            config,
            info,
            servers: None,
        })
    }

    pub fn plextv(&self) -> Result<PlexTvClient> {
        // Undocumented override so the CLI can be exercised against a fake plex.tv.
        let client = match std::env::var("HAUL_PLEX_TV_URL") {
            Ok(url) => PlexTvClient::with_base_url(self.info.clone(), &url)?,
            Err(_) => PlexTvClient::new(self.info.clone())?,
        };
        Ok(client.with_token(self.config.token()?))
    }

    /// Connect to `server`, starting from its cached connection.
    pub async fn connect(&self, server: &Resource) -> haul_core::Result<ServerClient> {
        let cached = self.config.connections.get(&server.client_identifier);
        ServerClient::connect(server, &self.info, cached.map(String::as_str)).await
    }

    /// The account's servers, fetched from plex.tv once per run.
    pub async fn servers(&mut self) -> Result<Vec<Resource>> {
        if let Some(servers) = &self.servers {
            return Ok(servers.clone());
        }
        let servers = self.plextv()?.servers().await?;
        self.servers = Some(servers.clone());
        Ok(servers)
    }

    /// Find a server by name or id and connect to it.
    pub async fn server(&mut self, query: &str) -> Result<ServerClient> {
        Ok(self.server_with_resource(query).await?.1)
    }

    /// Like [`server`](Self::server), also returning the server's record so
    /// it can be reconnected to later.
    pub async fn server_with_resource(&mut self, query: &str) -> Result<(Resource, ServerClient)> {
        let servers = self.servers().await?;
        let server = haul_core::find_server(&servers, query)?.clone();
        let client = self.connect(&server).await?;
        self.remember([(&server, &client)])?;
        Ok((server, client))
    }

    pub fn my_library_cache(&self) -> MyLibraryCache {
        MyLibraryCache::in_dir(self.store.path().parent().unwrap_or(".".as_ref()))
    }

    /// What's on your own servers, from a recent saved copy when there is
    /// one. `refresh` reads the servers again regardless.
    pub async fn my_library(&mut self, refresh: bool) -> Result<MyLibrary> {
        let servers = self.servers().await?;
        let mine = haul_core::my_servers(&servers, &self.config);
        if mine.is_empty() {
            bail!(
                "no server counts as your library: you don't own one. \
                 Pick servers with `haul my-library <SERVER>...`"
            );
        }
        let ids: Vec<&str> = mine.iter().map(|s| s.client_identifier.as_str()).collect();
        let cache = self.my_library_cache();
        if !refresh && let Some(lib) = cache.load(&ids, MY_LIBRARY_MAX_AGE) {
            return Ok(lib);
        }

        eprintln!("Reading your library…");
        let results = join_all(mine.iter().map(|s| self.connect(s))).await;
        let mut clients = Vec::new();
        for (server, result) in mine.iter().zip(results) {
            match result {
                Ok(client) => clients.push((*server, client)),
                Err(_) => eprintln!(
                    "warning: {} is unreachable; comparing without it",
                    server.name
                ),
            }
        }
        self.remember(clients.iter().map(|(s, c)| (*s, c)))?;
        let pairs: Vec<_> = clients.iter().map(|(s, c)| (*s, c)).collect();
        let lib = MyLibrary::load(&pairs).await;
        for name in &lib.skipped {
            eprintln!("warning: could not read {name}; comparing without it");
        }
        if lib.servers.is_empty() {
            bail!("could not read any of your own servers");
        }
        if let Err(e) = cache.save(&lib) {
            tracing::warn!("could not save your library for next time: {e}");
        }
        Ok(lib)
    }

    /// Whether `server` is one of the servers counted as your library.
    pub fn is_mine(&self, server: &Resource) -> bool {
        haul_core::is_mine(server, &self.config)
    }

    /// Remember the connections that worked so the next run skips probing.
    pub fn remember<'a>(
        &mut self,
        clients: impl IntoIterator<Item = (&'a Resource, &'a ServerClient)>,
    ) -> Result<()> {
        let mut changed = false;
        for (server, client) in clients {
            let uri = &client.reachable().connection.uri;
            if self.config.connections.get(&server.client_identifier) != Some(uri) {
                self.config
                    .connections
                    .insert(server.client_identifier.clone(), uri.clone());
                changed = true;
            }
        }
        if changed {
            self.store.save(&self.config)?;
        }
        Ok(())
    }
}

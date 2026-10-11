//! Shared setup for commands that talk to Plex: config, plex.tv client, server connections.

use anyhow::Result;
use plex_core::{ClientInfo, Config, ConfigStore, PlexTvClient, Resource, ServerClient};

pub struct Context {
    pub store: ConfigStore,
    pub config: Config,
    pub info: ClientInfo,
}

impl Context {
    pub fn load(store: ConfigStore) -> Result<Self> {
        let config = store.load_or_init()?;
        let info = ClientInfo::new(&config.client_id);
        Ok(Context {
            store,
            config,
            info,
        })
    }

    pub fn plextv(&self) -> Result<PlexTvClient> {
        Ok(PlexTvClient::new(self.info.clone())?.with_token(self.config.token()?))
    }

    /// Connect to `server`, starting from its cached connection.
    pub async fn connect(&self, server: &Resource) -> plex_core::Result<ServerClient> {
        let cached = self.config.connections.get(&server.client_identifier);
        ServerClient::connect(server, &self.info, cached.map(String::as_str)).await
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

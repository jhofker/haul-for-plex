//! Client for a single Plex Media Server, reached over its best working connection.

use std::time::{Duration, Instant};

use futures_util::future::join_all;
use serde::Deserialize;
use url::Url;

use crate::error::{Error, Result};
use crate::plextv::{ClientInfo, Connection, Resource};

/// How long a single connection probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// A connection that answered a probe.
#[derive(Debug, Clone)]
pub struct Reachable {
    pub connection: Connection,
    pub latency: Duration,
}

#[derive(Debug, Clone)]
pub struct ServerClient {
    http: reqwest::Client,
    base: Url,
    token: String,
    name: String,
    reachable: Reachable,
}

impl ServerClient {
    /// Connect to `server`, trying `preferred_uri` (a previously working
    /// connection) first, then probing every connection in parallel and
    /// picking the best: local, then remote, then relay.
    pub async fn connect(
        server: &Resource,
        info: &ClientInfo,
        preferred_uri: Option<&str>,
    ) -> Result<Self> {
        let token = server
            .access_token
            .clone()
            .ok_or_else(|| Error::Unreachable {
                server: server.name.clone(),
                attempts: vec![("plex.tv".into(), "no access token for this server".into())],
            })?;
        let http = reqwest::Client::builder()
            .default_headers(info.headers())
            .build()?;

        if let Some(conn) =
            preferred_uri.and_then(|uri| server.connections.iter().find(|c| c.uri == uri))
            && let Ok(latency) = probe(&http, server, &token, conn).await
        {
            return Self::new(http, token, server, conn.clone(), latency);
        }

        let results = join_all(
            server
                .connections
                .iter()
                .map(|conn| probe(&http, server, &token, conn)),
        )
        .await;

        let mut attempts = Vec::new();
        let mut best: Option<(&Connection, Duration)> = None;
        for (conn, result) in server.connections.iter().zip(results) {
            match result {
                Ok(latency) => {
                    let better = best
                        .is_none_or(|(b, b_latency)| (rank(conn), latency) < (rank(b), b_latency));
                    if better {
                        best = Some((conn, latency));
                    }
                }
                Err(e) => attempts.push((conn.uri.clone(), e)),
            }
        }

        match best {
            Some((conn, latency)) => Self::new(http, token, server, conn.clone(), latency),
            None => Err(Error::Unreachable {
                server: server.name.clone(),
                attempts,
            }),
        }
    }

    fn new(
        http: reqwest::Client,
        token: String,
        server: &Resource,
        connection: Connection,
        latency: Duration,
    ) -> Result<Self> {
        Ok(ServerClient {
            http,
            base: Url::parse(&connection.uri)?,
            token,
            name: server.name.clone(),
            reachable: Reachable {
                connection,
                latency,
            },
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn base_url(&self) -> &Url {
        &self.base
    }

    /// The connection in use and how quickly it answered.
    pub fn reachable(&self) -> &Reachable {
        &self.reachable
    }

    #[allow(dead_code)] // used by the browse/download milestones
    pub(crate) fn get(&self, path: &str) -> Result<reqwest::RequestBuilder> {
        Ok(self
            .http
            .get(self.base.join(path)?)
            .header("X-Plex-Token", &self.token))
    }
}

/// Lower is better: direct local, then direct remote, then relay.
fn rank(conn: &Connection) -> u8 {
    match (conn.relay, conn.local) {
        (false, true) => 0,
        (false, false) => 1,
        (true, _) => 2,
    }
}

#[derive(Deserialize)]
struct Identity {
    #[serde(rename = "MediaContainer")]
    container: IdentityContainer,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityContainer {
    machine_identifier: String,
}

/// Check that `conn` answers and really is `server` (a stale LAN address can
/// point at a different machine). Errors are returned as display strings for reporting.
async fn probe(
    http: &reqwest::Client,
    server: &Resource,
    token: &str,
    conn: &Connection,
) -> std::result::Result<Duration, String> {
    let url = Url::parse(&conn.uri)
        .and_then(|u| u.join("identity"))
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    let resp = http
        .get(url)
        .header("X-Plex-Token", token)
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| describe(&e))?;
    let identity: Identity = resp.json().await.map_err(|e| describe(&e))?;
    if identity.container.machine_identifier != server.client_identifier {
        return Err(format!(
            "answered as a different server ({})",
            identity.container.machine_identifier
        ));
    }
    Ok(start.elapsed())
}

fn describe(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".into()
    } else if e.is_connect() {
        "connection failed".into()
    } else if let Some(status) = e.status() {
        format!("HTTP {status}")
    } else {
        e.to_string()
    }
}

/// Pick a server by machine id, exact name, or unique case-insensitive name prefix.
pub fn find_server<'a>(servers: &'a [Resource], query: &str) -> Result<&'a Resource> {
    if let Some(s) = servers.iter().find(|s| s.client_identifier == query) {
        return Ok(s);
    }
    let q = query.to_lowercase();
    let exact: Vec<_> = servers
        .iter()
        .filter(|s| s.name.to_lowercase() == q)
        .collect();
    let candidates = if exact.is_empty() {
        servers
            .iter()
            .filter(|s| s.name.to_lowercase().starts_with(&q))
            .collect()
    } else {
        exact
    };
    match candidates.as_slice() {
        [one] => Ok(one),
        [] => Err(Error::ServerNotFound(query.into())),
        many => Err(Error::AmbiguousServer {
            query: query.into(),
            matches: many.iter().map(|s| s.name.clone()).collect(),
        }),
    }
}

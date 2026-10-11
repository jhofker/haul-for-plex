//! Client for the plex.tv account API: PIN login, user info and server discovery.

use std::time::Duration;

use reqwest::header::{ACCEPT, HeaderMap, HeaderValue};
use serde::Deserialize;
use url::Url;

use crate::error::{Error, Result};

const PLEX_TV: &str = "https://plex.tv/";
const AUTH_APP: &str = "https://app.plex.tv/auth";
/// Plex PINs live 30 minutes; used when the response omits `expiresIn`.
const DEFAULT_PIN_LIFETIME_SECS: u64 = 30 * 60;

/// Identifies this app to Plex; sent as `X-Plex-*` headers on every request.
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub client_id: String,
    pub product: String,
    pub version: String,
    pub platform: String,
    pub device_name: String,
}

impl ClientInfo {
    pub fn new(client_id: impl Into<String>) -> Self {
        ClientInfo {
            client_id: client_id.into(),
            product: crate::PRODUCT.into(),
            version: crate::VERSION.into(),
            platform: std::env::consts::OS.into(),
            device_name: crate::PRODUCT.into(),
        }
    }

    pub(crate) fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        for (name, value) in [
            ("X-Plex-Client-Identifier", &self.client_id),
            ("X-Plex-Product", &self.product),
            ("X-Plex-Version", &self.version),
            ("X-Plex-Platform", &self.platform),
            ("X-Plex-Device-Name", &self.device_name),
        ] {
            if let Ok(value) = HeaderValue::from_str(value) {
                headers.insert(name, value);
            }
        }
        headers
    }
}

/// A login PIN. `auth_token` is set once the user approves it in the browser.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    pub id: u64,
    pub code: String,
    pub auth_token: Option<String>,
    pub expires_at: Option<String>,
    /// Seconds until the PIN expires, as of when it was fetched.
    #[serde(default)]
    pub expires_in: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: u64,
    pub uuid: String,
    pub username: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub email: Option<String>,
}

/// A device on the account, e.g. a Plex Media Server you own or that is shared with you.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub name: String,
    pub client_identifier: String,
    #[serde(default)]
    pub product: String,
    #[serde(default)]
    pub product_version: String,
    #[serde(default)]
    pub platform: Option<String>,
    /// Comma-separated capabilities, e.g. `"server"` or `"client,player"`.
    #[serde(default)]
    pub provides: String,
    #[serde(default)]
    pub owned: bool,
    #[serde(default)]
    pub presence: bool,
    /// Token for talking to this server; differs from the account token on shared servers.
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub connections: Vec<Connection>,
}

impl Resource {
    pub fn is_server(&self) -> bool {
        self.provides.split(',').any(|p| p.trim() == "server")
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub protocol: String,
    pub address: String,
    pub port: u16,
    pub uri: String,
    #[serde(default)]
    pub local: bool,
    #[serde(default)]
    pub relay: bool,
    #[serde(default, rename = "IPv6")]
    pub ipv6: bool,
}

#[derive(Debug, Clone)]
pub struct PlexTvClient {
    http: reqwest::Client,
    base: Url,
    info: ClientInfo,
    token: Option<String>,
}

impl PlexTvClient {
    pub fn new(info: ClientInfo) -> Result<Self> {
        Self::with_base_url(info, PLEX_TV)
    }

    /// Point at a different plex.tv host (used by tests).
    pub fn with_base_url(info: ClientInfo, base: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .default_headers(info.headers())
            .build()?;
        Ok(PlexTvClient {
            http,
            base: Url::parse(base)?,
            info,
            token: None,
        })
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// Start a login: create a PIN the user approves at [`Self::auth_url`].
    pub async fn create_pin(&self) -> Result<Pin> {
        let url = self.base.join("api/v2/pins?strong=true")?;
        send(self.http.post(url)).await
    }

    /// Re-fetch a PIN; `auth_token` is filled in once the user has approved it.
    pub async fn check_pin(&self, id: u64) -> Result<Pin> {
        let url = self.base.join(&format!("api/v2/pins/{id}"))?;
        send(self.http.get(url)).await
    }

    /// Poll `pin` every `interval` until the user approves it, returning the auth token.
    ///
    /// Gives up when the PIN expires (Plex returns 404 or `expires_in` elapses).
    pub async fn wait_for_pin(&self, pin: &Pin, interval: Duration) -> Result<String> {
        let lifetime = Duration::from_secs(pin.expires_in.unwrap_or(DEFAULT_PIN_LIFETIME_SECS));
        let deadline = tokio::time::Instant::now() + lifetime;
        loop {
            match self.check_pin(pin.id).await {
                Ok(Pin {
                    auth_token: Some(token),
                    ..
                }) if !token.is_empty() => return Ok(token),
                Ok(_) => {}
                Err(Error::Status { status, .. }) if status == reqwest::StatusCode::NOT_FOUND => {
                    return Err(Error::PinExpired);
                }
                Err(e) => return Err(e),
            }
            if tokio::time::Instant::now() + interval >= deadline {
                return Err(Error::PinExpired);
            }
            tokio::time::sleep(interval).await;
        }
    }

    /// Browser URL where the user approves `pin`.
    pub fn auth_url(&self, pin: &Pin) -> String {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("clientID", &self.info.client_id)
            .append_pair("code", &pin.code)
            .append_pair("context[device][product]", &self.info.product)
            .finish();
        format!("{AUTH_APP}#?{query}")
    }

    pub async fn user(&self) -> Result<User> {
        let url = self.base.join("api/v2/user")?;
        send(self.authed(self.http.get(url))?).await
    }

    /// All Plex Media Servers the account can reach, owned or shared.
    pub async fn servers(&self) -> Result<Vec<Resource>> {
        let url = self
            .base
            .join("api/v2/resources?includeHttps=1&includeRelay=1&includeIPv6=1")?;
        let resources: Vec<Resource> = send(self.authed(self.http.get(url))?).await?;
        Ok(resources.into_iter().filter(Resource::is_server).collect())
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
        let token = self.token.as_deref().ok_or(Error::NotLoggedIn)?;
        Ok(req.header("X-Plex-Token", token))
    }
}

async fn send<T: serde::de::DeserializeOwned>(req: reqwest::RequestBuilder) -> Result<T> {
    let resp = req.send().await?;
    let status = resp.status();
    if !status.is_success() {
        let url = resp.url().to_string();
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Status { status, url, body });
    }
    Ok(resp.json().await?)
}

//! Persistent configuration: a stable client id plus the Plex auth token.
//!
//! Stored as JSON in `$HAUL_CONFIG_DIR/config.json` (or the OS config
//! dir). The file holds a Plex token, so it is written with `0600` on Unix.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const CONFIG_DIR_ENV: &str = "HAUL_CONFIG_DIR";
const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Sent as `X-Plex-Client-Identifier`; generated once and kept so Plex
    /// sees a single device across runs.
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<Auth>,
    /// Last working connection URI per server, keyed by machine identifier.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub connections: BTreeMap<String, String>,
    /// Machine ids of the servers that count as your own library. Empty
    /// means every server you own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub my_servers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Auth {
    pub token: String,
    pub username: String,
}

impl Config {
    pub fn new() -> Self {
        Config {
            client_id: uuid::Uuid::new_v4().to_string(),
            auth: None,
            connections: BTreeMap::new(),
            my_servers: Vec::new(),
        }
    }

    pub fn token(&self) -> Result<&str> {
        self.auth
            .as_ref()
            .map(|a| a.token.as_str())
            .ok_or(Error::NotLoggedIn)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads and writes [`Config`] at a fixed path.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    /// Store in `dir/config.json`.
    pub fn in_dir(dir: impl AsRef<Path>) -> Self {
        ConfigStore {
            path: dir.as_ref().join(CONFIG_FILE),
        }
    }

    /// Store in `$HAUL_CONFIG_DIR`, falling back to the OS config dir.
    pub fn from_env() -> Result<Self> {
        if let Some(dir) = std::env::var_os(CONFIG_DIR_ENV).filter(|d| !d.is_empty()) {
            return Ok(Self::in_dir(PathBuf::from(dir)));
        }
        let dirs = directories::ProjectDirs::from("", "", "haul").ok_or(Error::NoConfigDir)?;
        Ok(Self::in_dir(dirs.config_dir()))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the config, creating and saving a fresh one if none exists.
    pub fn load_or_init(&self) -> Result<Config> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| Error::Config {
                path: self.path.clone(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let config = Config::new();
                self.save(&config)?;
                Ok(config)
            }
            Err(e) => Err(Error::io(&self.path, e)),
        }
    }

    /// Atomically write the config (temp file + rename), owner-only on Unix.
    pub fn save(&self, config: &Config) -> Result<()> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;

        let tmp = self.path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(config).expect("config serializes");
        let mut file = open_private(&tmp).map_err(|e| Error::io(&tmp, e))?;
        file.write_all(&json)
            .and_then(|()| file.sync_all())
            .map_err(|e| Error::io(&tmp, e))?;
        fs::rename(&tmp, &self.path).map_err(|e| Error::io(&self.path, e))
    }
}

#[cfg(unix)]
fn open_private(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_private(path: &Path) -> std::io::Result<fs::File> {
    fs::File::create(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_and_reloads_same_client_id() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::in_dir(dir.path().join("nested"));

        let first = store.load_or_init().unwrap();
        assert!(store.path().exists());
        assert!(first.auth.is_none());

        let second = store.load_or_init().unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn save_round_trips_auth() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::in_dir(dir.path());
        let mut config = store.load_or_init().unwrap();
        assert!(matches!(config.token(), Err(Error::NotLoggedIn)));

        config.auth = Some(Auth {
            token: "tok".into(),
            username: "me".into(),
        });
        store.save(&config).unwrap();

        let loaded = store.load_or_init().unwrap();
        assert_eq!(loaded.token().unwrap(), "tok");
    }

    #[cfg(unix)]
    #[test]
    fn config_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::in_dir(dir.path());
        store.load_or_init().unwrap();
        let mode = fs::metadata(store.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn invalid_json_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::in_dir(dir.path());
        fs::write(store.path(), b"not json").unwrap();
        assert!(matches!(store.load_or_init(), Err(Error::Config { .. })));
    }
}

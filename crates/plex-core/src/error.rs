use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http request failed")]
    Http(#[from] reqwest::Error),

    #[error("{url} returned {status}: {body}")]
    Status {
        status: reqwest::StatusCode,
        url: String,
        body: String,
    },

    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),

    #[error("i/o error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid config file {path}: {source}")]
    Config {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("could not determine a config directory; set PLEX_GRABBER_CONFIG_DIR")]
    NoConfigDir,

    #[error("login PIN expired before it was approved; run `plex-grabber login` again")]
    PinExpired,

    #[error("could not reach server {server}:{}", format_attempts(.attempts))]
    Unreachable {
        server: String,
        attempts: Vec<(String, String)>,
    },

    #[error("no server matches {0:?}; run `plex-grabber servers` to list them")]
    ServerNotFound(String),

    #[error("{query:?} matches several servers: {}; be more specific", .matches.join(", "))]
    AmbiguousServer { query: String, matches: Vec<String> },

    #[error("no item with key {0} on this server")]
    ItemNotFound(String),

    #[error("not logged in; run `plex-grabber login` first")]
    NotLoggedIn,
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}

fn format_attempts(attempts: &[(String, String)]) -> String {
    if attempts.is_empty() {
        return " it has no connections".into();
    }
    attempts
        .iter()
        .map(|(uri, err)| format!("\n  {uri}: {err}"))
        .collect()
}

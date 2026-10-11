use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http request failed: {0}")]
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

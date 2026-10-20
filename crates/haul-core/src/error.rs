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

    #[error("could not determine a config directory; set HAUL_CONFIG_DIR")]
    NoConfigDir,

    #[error("login PIN expired before it was approved; run `haul login` again")]
    PinExpired,

    #[error("could not reach server {server}:{}", format_attempts(.attempts))]
    Unreachable {
        server: String,
        attempts: Vec<(String, String)>,
    },

    #[error("{title:?} has no version matching {wanted}; available: {}", .available.join(", "))]
    NoSuchVersion {
        title: String,
        wanted: String,
        available: Vec<String>,
    },

    #[error("no server matches {0:?}; run `haul servers` to list them")]
    ServerNotFound(String),

    #[error("{query:?} matches several servers: {}; be more specific", .matches.join(", "))]
    AmbiguousServer { query: String, matches: Vec<String> },

    #[error("no item with key {0} on this server")]
    ItemNotFound(String),

    #[error("{0} refused the download; its owner may have disabled downloads for your account")]
    DownloadForbidden(String),

    #[error("{} already exists with a different size; use --force to replace it", .0.display())]
    DestinationExists(PathBuf),

    #[error(
        "download ended at {actual} bytes but the server reported {expected}; the partial file was kept so it can resume"
    )]
    SizeMismatch { expected: u64, actual: u64 },

    #[error("{title:?} is a {kind}; only movies, episodes, seasons and shows can be downloaded")]
    NotDownloadable { title: String, kind: String },

    #[error("{0:?} has no media files")]
    NoMedia(String),

    #[error("not logged in; run `haul login` first")]
    NotLoggedIn,
}

impl Error {
    /// Whether trying again later might succeed: the server dropped, timed
    /// out or answered with a temporary error. Lost access (401/403), a
    /// removed item (404) and local problems such as a full disk are not.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Http(e) => match e.status() {
                Some(status) => retryable_status(status),
                None => !e.is_builder() && !e.is_redirect(),
            },
            Error::Status { status, .. } => retryable_status(*status),
            Error::Unreachable { .. } => true,
            // The transfer stopped early; the rest can be fetched with `Range`.
            Error::SizeMismatch { expected, actual } => actual < expected,
            _ => false,
        }
    }

    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}

fn retryable_status(status: reqwest::StatusCode) -> bool {
    use reqwest::StatusCode;
    status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
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

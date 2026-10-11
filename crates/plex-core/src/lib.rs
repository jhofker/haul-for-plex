//! Core library for plex-grabber: configuration, Plex API clients and downloads.

pub mod config;
pub mod error;
pub mod library;
pub mod plextv;
pub mod server;

pub use config::{Auth, Config, ConfigStore};
pub use error::{Error, Result};
pub use library::{Library, Media, Metadata, Part};
pub use plextv::{ClientInfo, PlexTvClient, Resource};
pub use server::{ServerClient, find_server};

/// Product name sent to Plex in `X-Plex-Product`.
pub const PRODUCT: &str = "plex-grabber";
/// Version sent to Plex in `X-Plex-Version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

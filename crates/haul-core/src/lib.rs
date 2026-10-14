#![doc = include_str!("../README.md")]

pub mod config;
pub mod download;
pub mod error;
pub mod library;
pub mod listing;
pub mod naming;
pub mod plextv;
pub mod server;

pub use config::{Auth, Config, ConfigStore};
pub use download::{Layout, Outcome, PlannedFile, Progress, VersionChoice};
pub use error::{Error, Result};
pub use library::{Library, Media, Metadata, Part};
pub use listing::{Filter, Sort, SortKey, View};
pub use plextv::{ClientInfo, PlexTvClient, Resource};
pub use server::{ServerClient, find_server};

/// Product name sent to Plex in `X-Plex-Product`.
pub const PRODUCT: &str = "Haul for Plex";
/// Version sent to Plex in `X-Plex-Version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Format a byte count with decimal units, e.g. `1.4 GB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

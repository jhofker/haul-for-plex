//! Plex-style folder and file names, safe on Linux, macOS and Windows.
//!
//! ```text
//! Movies/{Title} ({Year})/{Title} ({Year}).{ext}
//! TV/{Show} ({Year})/Season {NN}/{Show} - S{NN}E{NN} - {Episode Title}.{ext}
//! ```

use std::path::PathBuf;

/// Longest single path component we produce, in characters, before the extension.
const MAX_COMPONENT_CHARS: usize = 180;

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Make `name` usable as a single path component on every platform.
///
/// Colons become dashes (`Star Trek: Voyager` → `Star Trek - Voyager`), other
/// characters Windows forbids are replaced or dropped, and leading/trailing
/// dots and spaces are removed (which also neutralises `..`). Never returns an
/// empty string or a Windows reserved device name.
pub fn sanitize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.replace(": ", " - ").chars() {
        match c {
            ':' | '/' | '\\' | '|' => out.push('-'),
            '"' => out.push('\''),
            '<' | '>' | '*' | '?' => {}
            c if c.is_whitespace() => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let is_junk = |c: char| c == '.' || c.is_whitespace();
    let mut s: String = collapsed
        .trim_matches(is_junk)
        .chars()
        .take(MAX_COMPONENT_CHARS)
        .collect();
    s.truncate(s.trim_end_matches(is_junk).len());
    if s.is_empty() {
        return "_".into();
    }
    let stem = s.split('.').next().unwrap_or_default().to_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        s.insert(0, '_');
    }
    s
}

fn with_year(title: &str, year: Option<u32>) -> String {
    match year {
        Some(y) => format!("{title} ({y})"),
        None => title.to_string(),
    }
}

/// Keep only characters that are safe in an extension; `bin` if nothing is left.
fn clean_ext(ext: &str) -> String {
    let ext: String = ext
        .trim_start_matches('.')
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect::<String>()
        .to_lowercase();
    if ext.is_empty() { "bin".into() } else { ext }
}

/// `{stem}[ - {edition}][ - pt{N}].{ext}` — the part suffix only appears for
/// multi-part media, and the edition only for a non-default version (so e.g.
/// `Movie (2023) - 4K.mkv` sits beside `Movie (2023).mkv`, as Plex expects).
fn file_name(stem: &str, edition: Option<&str>, part: usize, parts: usize, ext: &str) -> String {
    let edition = match edition {
        Some(e) => format!(" - {e}"),
        None => String::new(),
    };
    let suffix = if parts > 1 {
        format!(" - pt{part}")
    } else {
        String::new()
    };
    format!("{}{edition}{suffix}.{}", sanitize(stem), clean_ext(ext))
}

/// Path of a movie file relative to the movies directory.
///
/// `part` is 1-based; `parts` is how many files the movie has.
///
/// `edition` names a non-default version, e.g. `4K`.
pub fn movie_path(
    title: &str,
    year: Option<u32>,
    edition: Option<&str>,
    part: usize,
    parts: usize,
    ext: &str,
) -> PathBuf {
    let name = with_year(title, year);
    PathBuf::from(sanitize(&name)).join(file_name(&name, edition, part, parts, ext))
}

/// What identifies an episode for naming.
#[derive(Debug, Clone, Copy)]
pub struct Episode<'a> {
    pub show: &'a str,
    /// Year the show started — not the episode's air year.
    pub show_year: Option<u32>,
    pub season: Option<u32>,
    pub number: Option<u32>,
    pub title: &'a str,
    /// Name of a non-default version, e.g. `4K`.
    pub edition: Option<&'a str>,
}

/// Path of an episode file relative to the TV directory.
///
/// Episodes without a season or episode number (some date-based shows) go
/// directly in the show folder as `{Show} - {Title}`.
pub fn episode_path(ep: &Episode<'_>, part: usize, parts: usize, ext: &str) -> PathBuf {
    let show_dir = sanitize(&with_year(ep.show, ep.show_year));
    match (ep.season, ep.number) {
        (Some(season), Some(number)) => {
            let stem = format!("{} - S{season:02}E{number:02} - {}", ep.show, ep.title);
            PathBuf::from(show_dir)
                .join(format!("Season {season:02}"))
                .join(file_name(&stem, ep.edition, part, parts, ext))
        }
        _ => {
            let stem = format!("{} - {}", ep.show, ep.title);
            PathBuf::from(show_dir).join(file_name(&stem, ep.edition, part, parts, ext))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn edition_goes_before_part_suffix() {
        assert_eq!(
            movie_path("Dune", Some(2021), Some("4K"), 1, 1, "mkv"),
            Path::new("Dune (2021)/Dune (2021) - 4K.mkv")
        );
        assert_eq!(
            movie_path("Dune", Some(2021), Some("4K"), 2, 2, "mkv"),
            Path::new("Dune (2021)/Dune (2021) - 4K - pt2.mkv")
        );
    }

    #[test]
    fn sanitize_replaces_forbidden_characters() {
        assert_eq!(sanitize("Star Trek: Voyager"), "Star Trek - Voyager");
        assert_eq!(sanitize("Re:Zero"), "Re-Zero");
        assert_eq!(sanitize("AC/DC: Live | 1991"), "AC-DC - Live - 1991");
        assert_eq!(sanitize(r#"Say "Hello"?"#), "Say 'Hello'");
        assert_eq!(sanitize("What <is> *this*"), "What is this");
    }

    #[test]
    fn sanitize_cleans_edges_and_whitespace() {
        assert_eq!(sanitize("  Lots   of   space  "), "Lots of space");
        assert_eq!(sanitize("Ellipsis..."), "Ellipsis");
        assert_eq!(sanitize("tab\there\u{0007}"), "tab here");
    }

    #[test]
    fn sanitize_cannot_escape_the_directory() {
        assert_eq!(sanitize(".."), "_");
        assert_eq!(sanitize("../../etc/passwd"), "-..-etc-passwd");
        assert_eq!(sanitize(""), "_");
        for name in ["..", ".", "a/../b", "/abs", "C:\\x"] {
            let s = sanitize(name);
            assert!(!s.contains('/') && !s.contains('\\') && s != "..", "{s}");
        }
    }

    #[test]
    fn sanitize_avoids_windows_device_names() {
        assert_eq!(sanitize("CON"), "_CON");
        assert_eq!(sanitize("nul"), "_nul");
        assert_eq!(sanitize("aux.txt"), "_aux.txt");
        assert_eq!(sanitize("Console"), "Console");
    }

    #[test]
    fn sanitize_keeps_unicode_and_limits_length() {
        assert_eq!(sanitize("Amélie (2001)"), "Amélie (2001)");
        assert_eq!(sanitize("千と千尋の神隠し"), "千と千尋の神隠し");
        let long = "x".repeat(500);
        assert_eq!(sanitize(&long).chars().count(), MAX_COMPONENT_CHARS);
    }

    #[test]
    fn movie_layout() {
        assert_eq!(
            movie_path("The Matrix", Some(1999), None, 1, 1, "mkv"),
            Path::new("The Matrix (1999)/The Matrix (1999).mkv")
        );
        assert_eq!(
            movie_path("Unknown Year", None, None, 1, 1, ".MP4"),
            Path::new("Unknown Year/Unknown Year.mp4")
        );
    }

    #[test]
    fn multi_part_movies_get_part_suffixes() {
        assert_eq!(
            movie_path("Epic", Some(1960), None, 2, 2, "avi"),
            Path::new("Epic (1960)/Epic (1960) - pt2.avi")
        );
    }

    #[test]
    fn episode_layout() {
        let ep = Episode {
            show: "The Office (US)",
            show_year: Some(2005),
            season: Some(6),
            number: Some(16),
            title: "The Manager and the Salesman",
            edition: None,
        };
        assert_eq!(
            episode_path(&ep, 1, 1, "mkv"),
            Path::new(
                "The Office (US) (2005)/Season 06/The Office (US) - S06E16 - The Manager and the Salesman.mkv"
            )
        );
    }

    #[test]
    fn specials_go_in_season_zero() {
        let ep = Episode {
            show: "Doctor Who",
            show_year: Some(2005),
            season: Some(0),
            number: Some(1),
            title: "Christmas: Invasion",
            edition: None,
        };
        assert_eq!(
            episode_path(&ep, 1, 1, "mkv"),
            Path::new("Doctor Who (2005)/Season 00/Doctor Who - S00E01 - Christmas - Invasion.mkv")
        );
    }

    #[test]
    fn episodes_without_numbers_land_in_the_show_folder() {
        let ep = Episode {
            show: "The Daily Show",
            show_year: None,
            season: None,
            number: None,
            title: "Some Guest",
            edition: None,
        };
        assert_eq!(
            episode_path(&ep, 1, 1, "mp4"),
            Path::new("The Daily Show/The Daily Show - Some Guest.mp4")
        );
    }

    #[test]
    fn extension_is_cleaned() {
        assert_eq!(clean_ext("mkv"), "mkv");
        assert_eq!(clean_ext(".MKV"), "mkv");
        assert_eq!(clean_ext("../x"), "x");
        assert_eq!(clean_ext(""), "bin");
    }
}

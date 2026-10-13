//! The filter and sort controls above lists, as they arrive in a query string.

use haul_core::{Filter, Sort, View};
use serde::Deserialize;

/// Raw form values; blank or invalid ones are ignored rather than rejected,
/// so a hand-edited URL still shows the list.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListParams {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub year: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub sort: String,
}

impl ListParams {
    pub fn view(&self) -> View {
        let text = self.title.trim();
        let kind = self.kind.trim();
        View {
            filter: Filter {
                text: (!text.is_empty()).then(|| text.to_string()),
                kind: (!kind.is_empty()).then(|| kind.to_ascii_lowercase()),
                year: self.year.trim().parse().ok(),
            },
            sort: self.sort(),
        }
    }

    pub fn sort(&self) -> Option<Sort> {
        self.sort.parse().ok()
    }

    /// The sort as the form's `<select>` names it; empty for the list's own order.
    pub fn sort_value(&self) -> String {
        self.sort().map(|s| s.to_string()).unwrap_or_default()
    }

    /// True when the list is shown as-is.
    pub fn is_default(&self) -> bool {
        self.view() == View::default()
    }

    /// Query-string pairs that reproduce these settings, for links such as
    /// the pager's. Defaults are left out.
    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        let view = self.view();
        let mut pairs = Vec::new();
        if let Some(text) = view.filter.text {
            pairs.push(("title", text));
        }
        if let Some(year) = view.filter.year {
            pairs.push(("year", year.to_string()));
        }
        if let Some(kind) = view.filter.kind {
            pairs.push(("type", kind));
        }
        if let Some(sort) = view.sort {
            pairs.push(("sort", sort.to_string()));
        }
        pairs
    }
}

/// Build `path?a=b&c=d`, or just `path` when there are no pairs.
pub fn href(path: &str, pairs: &[(&str, String)]) -> String {
    if pairs.is_empty() {
        return path.to_string();
    }
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        query.append_pair(k, v);
    }
    format!("{path}?{}", query.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_and_bad_values_are_ignored() {
        let p = ListParams {
            title: "  ".into(),
            year: "soon".into(),
            kind: String::new(),
            sort: "size".into(),
        };
        assert!(p.is_default());
        assert!(p.pairs().is_empty());
    }

    #[test]
    fn pairs_round_trip_into_links() {
        let p = ListParams {
            title: " star wars ".into(),
            year: "1977".into(),
            kind: "Movie".into(),
            sort: "YEAR:desc".into(),
        };
        assert_eq!(p.sort_value(), "year:desc");
        assert_eq!(
            href("/x", &p.pairs()),
            "/x?title=star+wars&year=1977&type=movie&sort=year%3Adesc"
        );
        assert_eq!(href("/x", &[]), "/x");
    }
}

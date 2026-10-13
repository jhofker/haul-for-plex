//! Filtering and sorting lists of items. Whole libraries are paged, so the
//! Plex server does the work for them; shorter lists (a season's episodes,
//! search results) are filtered and sorted here.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use crate::library::{Metadata, matches_title, tokens};

/// A field to sort items by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    /// Title, ignoring a leading "The" etc. where the server says so.
    Title,
    Year,
    /// When the item was added to the library.
    Added,
    /// Release or air date.
    Released,
    /// Audience rating.
    Rating,
}

impl SortKey {
    pub const ALL: [SortKey; 5] = [
        SortKey::Title,
        SortKey::Year,
        SortKey::Added,
        SortKey::Released,
        SortKey::Rating,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SortKey::Title => "title",
            SortKey::Year => "year",
            SortKey::Added => "added",
            SortKey::Released => "released",
            SortKey::Rating => "rating",
        }
    }

    /// The field name Plex's `sort` parameter takes.
    fn plex_field(self) -> &'static str {
        match self {
            SortKey::Title => "titleSort",
            SortKey::Year => "year",
            SortKey::Added => "addedAt",
            SortKey::Released => "originallyAvailableAt",
            SortKey::Rating => "audienceRating",
        }
    }
}

/// An order for a list, written `KEY` or `KEY:asc` for ascending and
/// `KEY:desc` for descending, e.g. `year:desc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub desc: bool,
}

impl FromStr for Sort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim().to_ascii_lowercase();
        let (name, dir) = s.split_once(':').unwrap_or((&s, "asc"));
        let key = SortKey::ALL
            .into_iter()
            .find(|k| k.name() == name)
            .ok_or_else(|| {
                let names: Vec<_> = SortKey::ALL.iter().map(|k| k.name()).collect();
                format!(
                    "unknown sort {name:?}; use one of {}, optionally with :asc or :desc",
                    names.join(", ")
                )
            })?;
        let desc = match dir {
            "asc" => false,
            "desc" => true,
            other => return Err(format!("unknown sort direction {other:?}; use asc or desc")),
        };
        Ok(Sort { key, desc })
    }
}

impl fmt::Display for Sort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key.name())?;
        if self.desc {
            f.write_str(":desc")?;
        }
        Ok(())
    }
}

impl Sort {
    /// The value for Plex's `sort` parameter, e.g. `addedAt:desc`.
    pub fn plex_param(self) -> String {
        let dir = if self.desc { "desc" } else { "asc" };
        format!("{}:{dir}", self.key.plex_field())
    }

    /// Sort items in place. Items missing the field go last either way;
    /// ties keep their original order.
    pub fn apply(self, items: &mut [Metadata]) {
        items.sort_by(|a, b| self.compare(a, b));
    }

    fn compare(self, a: &Metadata, b: &Metadata) -> Ordering {
        match self.key {
            SortKey::Title => self.order(Some(title_key(a)), Some(title_key(b))),
            SortKey::Year => self.order(a.year, b.year),
            SortKey::Added => self.order(a.added_at, b.added_at),
            SortKey::Released => self.order(
                a.originally_available_at.as_deref(),
                b.originally_available_at.as_deref(),
            ),
            SortKey::Rating => self.order(a.audience_rating, b.audience_rating),
        }
    }

    fn order<T: PartialOrd>(self, a: Option<T>, b: Option<T>) -> Ordering {
        match (a, b) {
            (Some(a), Some(b)) => {
                let o = a.partial_cmp(&b).unwrap_or(Ordering::Equal);
                if self.desc { o.reverse() } else { o }
            }
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }
}

fn title_key(m: &Metadata) -> String {
    m.title_sort.as_deref().unwrap_or(&m.title).to_lowercase()
}

/// Which items to keep. Empty fields keep everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Words the title must contain (for episodes and seasons, the show's
    /// title counts too).
    pub text: Option<String>,
    /// Item type: `movie`, `show`, `season` or `episode`.
    pub kind: Option<String>,
    pub year: Option<u32>,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.kind.is_none() && self.year.is_none()
    }

    pub fn matches(&self, m: &Metadata) -> bool {
        if let Some(text) = &self.text
            && !matches_title(m, &tokens(text))
        {
            return false;
        }
        if let Some(kind) = &self.kind
            && !m.kind.eq_ignore_ascii_case(kind)
        {
            return false;
        }
        if let Some(year) = self.year
            && m.year != Some(year)
        {
            return false;
        }
        true
    }
}

/// How to show a list: which items, in what order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    pub filter: Filter,
    /// `None` keeps the list's own order.
    pub sort: Option<Sort>,
}

impl View {
    /// Filter and sort a list that has already been fetched.
    pub fn apply(&self, items: &mut Vec<Metadata>) {
        if !self.filter.is_empty() {
            items.retain(|m| self.filter.matches(m));
        }
        if let Some(sort) = self.sort {
            sort.apply(items);
        }
    }

    /// Query parameters asking a Plex server to filter and sort a library
    /// listing. Plex matches `title` anywhere in the title, ignoring case.
    /// The type filter isn't sent: a library listing has one type anyway.
    pub(crate) fn plex_params(&self) -> Vec<(&'static str, String)> {
        let mut params = Vec::new();
        if let Some(sort) = self.sort {
            params.push(("sort", sort.plex_param()));
        }
        if let Some(text) = &self.filter.text {
            params.push(("title", text.clone()));
        }
        if let Some(year) = self.filter.year {
            params.push(("year", year.to_string()));
        }
        params
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(json: serde_json::Value) -> Metadata {
        let mut value = serde_json::json!({"ratingKey": "1", "type": "movie", "title": "x"});
        value
            .as_object_mut()
            .unwrap()
            .extend(json.as_object().unwrap().clone());
        serde_json::from_value(value).unwrap()
    }

    fn titles(items: &[Metadata]) -> Vec<&str> {
        items.iter().map(|m| m.title.as_str()).collect()
    }

    #[test]
    fn parses_and_prints_sorts() {
        let s: Sort = "year:desc".parse().unwrap();
        assert_eq!(
            s,
            Sort {
                key: SortKey::Year,
                desc: true
            }
        );
        assert_eq!(s.to_string(), "year:desc");
        assert_eq!(s.plex_param(), "year:desc");
        let s: Sort = "Title".parse().unwrap();
        assert_eq!(s.to_string(), "title");
        assert_eq!(s.plex_param(), "titleSort:asc");
        assert_eq!("added:asc".parse::<Sort>().unwrap().to_string(), "added");
        assert!("size".parse::<Sort>().is_err());
        assert!("year:up".parse::<Sort>().is_err());
    }

    #[test]
    fn sorts_with_missing_values_last() {
        let mut items = vec![
            item(serde_json::json!({"title": "B", "year": 2001})),
            item(serde_json::json!({"title": "None"})),
            item(serde_json::json!({"title": "A", "year": 1999})),
        ];
        let by_year = |desc| Sort {
            key: SortKey::Year,
            desc,
        };
        by_year(false).apply(&mut items);
        assert_eq!(titles(&items), ["A", "B", "None"]);
        by_year(true).apply(&mut items);
        assert_eq!(titles(&items), ["B", "A", "None"]);
    }

    #[test]
    fn title_sort_uses_sort_title_and_ignores_case() {
        let mut items = vec![
            item(serde_json::json!({"title": "The Zebra", "titleSort": "Zebra"})),
            item(serde_json::json!({"title": "apple"})),
            item(serde_json::json!({"title": "Mango"})),
        ];
        "title".parse::<Sort>().unwrap().apply(&mut items);
        assert_eq!(titles(&items), ["apple", "Mango", "The Zebra"]);
    }

    #[test]
    fn filters_by_text_type_and_year() {
        let mut items = vec![
            item(serde_json::json!({"title": "Spider-Man", "year": 2002})),
            item(serde_json::json!({"title": "Spider-Man 2", "year": 2004})),
            item(serde_json::json!({"title": "Pilot", "type": "episode",
                                    "grandparentTitle": "Spider-Man Show"})),
            item(serde_json::json!({"title": "Hackers", "year": 1995})),
        ];
        let view = |text: &str, kind: Option<&str>, year| View {
            filter: Filter {
                text: Some(text.into()),
                kind: kind.map(String::from),
                year,
            },
            sort: None,
        };
        let mut all = items.clone();
        view("spider man", None, None).apply(&mut all);
        assert_eq!(titles(&all), ["Spider-Man", "Spider-Man 2", "Pilot"]);
        let mut movies = items.clone();
        view("spiderman", Some("movie"), None).apply(&mut movies);
        assert_eq!(titles(&movies), ["Spider-Man", "Spider-Man 2"]);
        view("spider", None, Some(2004)).apply(&mut items);
        assert_eq!(titles(&items), ["Spider-Man 2"]);
    }

    #[test]
    fn plex_params_cover_sort_title_and_year() {
        let view = View {
            filter: Filter {
                text: Some("alien".into()),
                kind: Some("movie".into()),
                year: Some(1979),
            },
            sort: Some("added:desc".parse().unwrap()),
        };
        assert_eq!(
            view.plex_params(),
            [
                ("sort", "addedAt:desc".to_string()),
                ("title", "alien".to_string()),
                ("year", "1979".to_string()),
            ]
        );
        assert!(View::default().plex_params().is_empty());
    }
}

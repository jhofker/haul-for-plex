use anyhow::Result;
use haul_core::{Library, Metadata, MyLibrary, Status, StatusFilter, View};

use crate::context::Context;
use crate::table;

pub async fn libraries(mut ctx: Context, server: &str, json: bool) -> Result<()> {
    let client = ctx.server(server).await?;
    let libs = client.libraries().await?;
    if json {
        let entries: Vec<_> = libs
            .iter()
            .map(|l| serde_json::json!({"key": l.key, "type": l.kind, "title": l.title}))
            .collect();
        crate::output::json(&entries);
        return Ok(());
    }
    let rows: Vec<[String; 3]> = libs
        .iter()
        .map(|l| [l.key.clone(), l.kind.clone(), l.title.clone()])
        .collect();
    table::print(["KEY", "TYPE", "TITLE"], &rows);
    Ok(())
}

/// Whether and how to compare a listing with your own library.
#[derive(Debug, Clone, Copy, Default)]
pub struct Compare {
    /// Show each item's status.
    pub enabled: bool,
    /// Only list items with this status (implies `enabled`).
    pub status: Option<StatusFilter>,
}

impl Compare {
    pub fn wanted(self) -> bool {
        self.enabled || self.status.is_some()
    }
}

/// List a library (by name or key) or the contents/details of an item (by key).
/// `view` filters and sorts the listing; it doesn't apply to a single item's details.
pub async fn ls(
    mut ctx: Context,
    server: &str,
    target: &str,
    view: &View,
    compare: Compare,
    json: bool,
) -> Result<()> {
    let client = ctx.server(server).await?;
    let mine = if compare.wanted() {
        Some(ctx.my_library(false).await?)
    } else {
        None
    };

    let libs = client.libraries().await?;
    if let Some(lib) = find_library(&libs, target) {
        let items = client.library_items(&lib.key, view).await?;
        let (items, statuses) = marked(items, mine.as_ref(), compare.status, |m, i| m.status(i));
        if json {
            crate::output::json(&items_json(&items, statuses.as_deref()));
            return Ok(());
        }
        print_items(&items, statuses.as_deref());
        hint(server, &items);
        return Ok(());
    }

    let item = client.metadata(target).await?;
    let status = mine.as_ref().and_then(|m| m.status(&item));
    if item.is_playable() {
        if json {
            let mut value = item_json(&item);
            add_status(&mut value, status);
            crate::output::json(&value);
        } else {
            print_details(&item);
            if let Some(status) = status {
                println!("\nyour library: {status}");
            }
        }
    } else {
        let mut children = client.children(&item.rating_key).await?;
        view.apply(&mut children);
        let (children, statuses) = marked(children, mine.as_ref(), compare.status, |m, c| {
            m.status_under(&item, c)
        });
        if json {
            let mut value = item_json(&item);
            add_status(&mut value, status);
            value["children"] = items_json(&children, statuses.as_deref()).into();
            crate::output::json(&value);
            return Ok(());
        }
        println!("{}", describe(&item));
        if let Some(status) = status {
            println!("your library: {status}");
        }
        println!();
        print_items(&children, statuses.as_deref());
        hint(server, &children);
    }
    Ok(())
}

/// Each item's status in `mine` (when comparing), keeping only those that
/// pass `filter`.
pub fn marked(
    items: Vec<Metadata>,
    mine: Option<&MyLibrary>,
    filter: Option<StatusFilter>,
    status: impl Fn(&MyLibrary, &Metadata) -> Option<Status>,
) -> (Vec<Metadata>, Option<Vec<Option<Status>>>) {
    let Some(mine) = mine else {
        return (items, None);
    };
    let (items, statuses) = items
        .into_iter()
        .map(|m| {
            let s = status(mine, &m);
            (m, s)
        })
        .filter(|(_, s)| filter.is_none_or(|f| f.matches(*s)))
        .unzip();
    (items, Some(statuses))
}

pub fn add_status(value: &mut serde_json::Value, status: Option<Status>) {
    if let Some(status) = status {
        value["status"] = status.name().into();
        value["status_label"] = status.label().into();
        if let Status::Partial { missing } = status {
            value["missing_episodes"] = missing.into();
        }
    }
}

fn items_json(items: &[Metadata], statuses: Option<&[Option<Status>]>) -> Vec<serde_json::Value> {
    items
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut value = item_json(m);
            add_status(&mut value, statuses.and_then(|s| s[i]));
            value
        })
        .collect()
}

/// An item as JSON, including its files when they are known.
fn item_json(m: &Metadata) -> serde_json::Value {
    let files: Vec<_> = m
        .media
        .first()
        .map(|media| {
            media
                .parts
                .iter()
                .map(|p| serde_json::json!({"file": p.file, "size": p.size, "container": p.container}))
                .collect()
        })
        .unwrap_or_default();
    serde_json::json!({
        "key": m.rating_key,
        "type": m.kind,
        "title": m.title,
        "year": m.year,
        "description": describe(m),
        "season": m.parent_index,
        "episode": m.index,
        "show": m.grandparent_title,
        "files": files,
    })
}

/// Match a library by case-insensitive title (exact, then unique prefix).
/// Keys aren't matched: they would collide with item keys.
fn find_library<'a>(libs: &'a [Library], target: &str) -> Option<&'a Library> {
    let t = target.to_lowercase();
    libs.iter()
        .find(|l| l.title.to_lowercase() == t)
        .or_else(|| {
            let mut prefixed = libs
                .iter()
                .filter(|l| l.title.to_lowercase().starts_with(&t));
            match (prefixed.next(), prefixed.next()) {
                (Some(one), None) => Some(one),
                _ => None,
            }
        })
}

fn print_items(items: &[Metadata], statuses: Option<&[Option<Status>]>) {
    if items.is_empty() {
        println!("(empty)");
        return;
    }
    match statuses {
        None => {
            let rows: Vec<[String; 3]> = items
                .iter()
                .map(|m| [m.rating_key.clone(), m.kind.clone(), describe(m)])
                .collect();
            table::print(["KEY", "TYPE", "TITLE"], &rows);
        }
        Some(statuses) => {
            let rows: Vec<[String; 4]> = items
                .iter()
                .zip(statuses)
                .map(|(m, s)| {
                    [
                        m.rating_key.clone(),
                        m.kind.clone(),
                        status_cell(*s),
                        describe(m),
                    ]
                })
                .collect();
            table::print(["KEY", "TYPE", "YOUR LIBRARY", "TITLE"], &rows);
        }
    }
}

/// A status for a table cell; blank for things that can't be compared.
pub fn status_cell(status: Option<Status>) -> String {
    status.map(Status::label).unwrap_or_default()
}

fn hint(server: &str, items: &[Metadata]) {
    if let Some(first) = items.first() {
        println!(
            "\nRun `haul ls {server:?} {}` to look inside an item.",
            first.rating_key
        );
    }
}

/// One-line description of an item, e.g. `Some Show - S01E02 - Title`.
pub fn describe(m: &Metadata) -> String {
    let year = |y: Option<u32>| y.map(|y| format!(" ({y})")).unwrap_or_default();
    match m.kind.as_str() {
        "show" => {
            let mut s = format!("{}{}", m.title, year(m.year));
            if let (Some(seasons), Some(eps)) = (m.child_count, m.leaf_count) {
                s += &format!(" - {seasons} seasons, {eps} episodes");
            }
            s
        }
        "season" => {
            let mut s = m.title.clone();
            if let Some(show) = &m.parent_title {
                s = format!("{show} - {s}");
            }
            if let Some(eps) = m.leaf_count {
                s += &format!(" ({eps} episodes)");
            }
            s
        }
        "episode" => {
            let code = match (m.parent_index, m.index) {
                (Some(season), Some(ep)) => format!("S{season:02}E{ep:02} - "),
                _ => String::new(),
            };
            match &m.grandparent_title {
                Some(show) => format!("{show} - {code}{}", m.title),
                None => format!("{code}{}", m.title),
            }
        }
        _ => format!("{}{}", m.title, year(m.year)),
    }
}

fn print_details(item: &Metadata) {
    println!("{}", describe(item));
    println!("key: {}", item.rating_key);
    for (i, media) in item.media.iter().enumerate() {
        let mut info = vec![];
        if let Some(r) = &media.video_resolution {
            info.push(if r.chars().all(|c| c.is_ascii_digit()) {
                format!("{r}p")
            } else {
                r.clone()
            });
        }
        info.extend(media.video_codec.clone());
        info.extend(media.container.clone());
        println!("\nversion {}: {}", i + 1, info.join(", "));
        for part in &media.parts {
            let name = part
                .file
                .as_deref()
                .and_then(|f| f.rsplit(['/', '\\']).next())
                .unwrap_or(&part.key);
            let size = part
                .size
                .map(table::size)
                .unwrap_or_else(|| "unknown size".into());
            println!("  {name} ({size})");
        }
    }
}

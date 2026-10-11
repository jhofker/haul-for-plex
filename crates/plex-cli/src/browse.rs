use anyhow::Result;
use plex_core::{Library, Metadata};

use crate::context::Context;
use crate::table;

pub async fn libraries(mut ctx: Context, server: &str) -> Result<()> {
    let client = ctx.server(server).await?;
    let libs = client.libraries().await?;
    let rows: Vec<[String; 3]> = libs
        .iter()
        .map(|l| [l.key.clone(), l.kind.clone(), l.title.clone()])
        .collect();
    table::print(["KEY", "TYPE", "TITLE"], &rows);
    Ok(())
}

/// List a library (by name or key) or the contents/details of an item (by key).
pub async fn ls(mut ctx: Context, server: &str, target: &str) -> Result<()> {
    let client = ctx.server(server).await?;

    let libs = client.libraries().await?;
    if let Some(lib) = find_library(&libs, target) {
        let items = client.library_items(&lib.key).await?;
        print_items(&items);
        hint(server, &items);
        return Ok(());
    }

    let item = client.metadata(target).await?;
    if item.is_playable() {
        print_details(&item);
    } else {
        let children = client.children(&item.rating_key).await?;
        println!("{}\n", describe(&item));
        print_items(&children);
        hint(server, &children);
    }
    Ok(())
}

/// Match a library by exact key, or by case-insensitive title (exact, then unique prefix).
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

fn print_items(items: &[Metadata]) {
    if items.is_empty() {
        println!("(empty)");
        return;
    }
    let rows: Vec<[String; 3]> = items
        .iter()
        .map(|m| [m.rating_key.clone(), m.kind.clone(), describe(m)])
        .collect();
    table::print(["KEY", "TYPE", "TITLE"], &rows);
}

fn hint(server: &str, items: &[Metadata]) {
    if let Some(first) = items.first() {
        println!(
            "\nRun `plex-grabber ls {server:?} {}` to look inside an item.",
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

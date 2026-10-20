use anyhow::Result;
use futures_util::future::join_all;

use crate::browse::{Compare, add_status, describe, marked, status_cell};
use crate::context::Context;
use crate::table;

/// Search one server, or every reachable server at once. When comparing
/// with your library, searching everywhere leaves out your own servers.
pub async fn search(
    mut ctx: Context,
    query: &str,
    server: Option<&str>,
    limit: usize,
    compare: Compare,
    json: bool,
) -> Result<()> {
    let servers = ctx.servers().await?;
    let targets = match server {
        Some(q) => vec![haul_core::find_server(&servers, q)?],
        None if compare.wanted() => servers.iter().filter(|s| !ctx.is_mine(s)).collect(),
        None => servers.iter().collect(),
    };
    let mine = if compare.wanted() {
        Some(ctx.my_library(false).await?)
    } else {
        None
    };

    let connections = join_all(targets.iter().map(|s| ctx.connect(s))).await;
    let mut clients = Vec::new();
    for (server, result) in targets.iter().zip(connections) {
        match result {
            Ok(client) => clients.push((*server, client)),
            // A single named server that can't be reached is an error; when
            // searching everywhere, skip it with a warning.
            Err(e) if targets.len() == 1 => return Err(e.into()),
            Err(_) => eprintln!("warning: skipping {} (unreachable)", server.name),
        }
    }
    ctx.remember(clients.iter().map(|(s, c)| (*s, c)))?;

    let results = join_all(clients.iter().map(|(_, c)| c.search(query, limit))).await;
    let mut rows = Vec::new();
    for ((server, _), result) in clients.iter().zip(results) {
        match result {
            Ok(items) => {
                let (items, statuses) =
                    marked(items, mine.as_ref(), compare.status, |m, i| m.status(i));
                rows.extend(items.iter().enumerate().map(|(i, m)| Row {
                    server: server.name.clone(),
                    key: m.rating_key.clone(),
                    kind: m.kind.clone(),
                    title: describe(m),
                    library: m.library_section_title.clone().unwrap_or_default(),
                    status: statuses.as_ref().and_then(|s| s[i]),
                }));
            }
            Err(e) => eprintln!("warning: search failed on {}: {e}", server.name),
        }
    }

    if json {
        let entries: Vec<_> = rows
            .iter()
            .map(|r| {
                let mut value = serde_json::json!({
                    "server": r.server, "key": r.key, "type": r.kind,
                    "title": r.title, "library": r.library,
                });
                add_status(&mut value, r.status);
                value
            })
            .collect();
        crate::output::json(&entries);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No results for {query:?}.");
        return Ok(());
    }
    if mine.is_some() {
        let cells: Vec<[String; 6]> = rows
            .into_iter()
            .map(|r| {
                [
                    r.server,
                    r.key,
                    r.kind,
                    status_cell(r.status),
                    r.title,
                    r.library,
                ]
            })
            .collect();
        table::print(
            ["SERVER", "KEY", "TYPE", "YOUR LIBRARY", "TITLE", "LIBRARY"],
            &cells,
        );
    } else {
        let cells: Vec<[String; 5]> = rows
            .into_iter()
            .map(|r| [r.server, r.key, r.kind, r.title, r.library])
            .collect();
        table::print(["SERVER", "KEY", "TYPE", "TITLE", "LIBRARY"], &cells);
    }
    println!("\nRun `haul ls <SERVER> <KEY>` for details.");
    Ok(())
}

struct Row {
    server: String,
    key: String,
    kind: String,
    title: String,
    library: String,
    status: Option<haul_core::Status>,
}

use anyhow::Result;
use futures_util::future::join_all;

use crate::browse::describe;
use crate::context::Context;
use crate::table;

/// Search one server, or every reachable server at once.
pub async fn search(
    mut ctx: Context,
    query: &str,
    server: Option<&str>,
    limit: usize,
    json: bool,
) -> Result<()> {
    let servers = ctx.plextv()?.servers().await?;
    let targets = match server {
        Some(q) => vec![haul_core::find_server(&servers, q)?],
        None => servers.iter().collect(),
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
            Ok(items) => rows.extend(items.iter().map(|m| {
                [
                    server.name.clone(),
                    m.rating_key.clone(),
                    m.kind.clone(),
                    describe(m),
                    m.library_section_title.clone().unwrap_or_default(),
                ]
            })),
            Err(e) => eprintln!("warning: search failed on {}: {e}", server.name),
        }
    }

    if json {
        let entries: Vec<_> = rows
            .iter()
            .map(|[server, key, kind, title, library]| {
                serde_json::json!({
                    "server": server, "key": key, "type": kind,
                    "title": title, "library": library,
                })
            })
            .collect();
        crate::output::json(&entries);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No results for {query:?}.");
        return Ok(());
    }
    table::print(["SERVER", "KEY", "TYPE", "TITLE", "LIBRARY"], &rows);
    println!("\nRun `haul ls <SERVER> <KEY>` for details.");
    Ok(())
}

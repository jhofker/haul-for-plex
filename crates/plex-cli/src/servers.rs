use anyhow::Result;
use futures_util::future::join_all;
use plex_core::Error;

use crate::context::Context;
use crate::table;

pub async fn list(mut ctx: Context, verbose: bool) -> Result<()> {
    let servers = ctx.plextv()?.servers().await?;
    if servers.is_empty() {
        println!("No servers found on this Plex account.");
        return Ok(());
    }

    let results = join_all(servers.iter().map(|s| ctx.connect(s))).await;

    let rows: Vec<[String; 4]> = servers
        .iter()
        .zip(&results)
        .map(|(server, result)| {
            let owner = if server.owned {
                "you".to_string()
            } else {
                server
                    .source_title
                    .clone()
                    .unwrap_or_else(|| "shared".into())
            };
            let status = match result {
                Ok(client) => {
                    let r = client.reachable();
                    let kind = match (r.connection.relay, r.connection.local) {
                        (true, _) => "relay",
                        (false, true) => "local",
                        (false, false) => "remote",
                    };
                    format!("{kind}, {} ms", r.latency.as_millis())
                }
                Err(_) => "unreachable".into(),
            };
            [
                server.name.clone(),
                owner,
                server.product_version.clone(),
                status,
            ]
        })
        .collect();
    table::print(["NAME", "OWNER", "VERSION", "CONNECTION"], &rows);

    if verbose {
        for (server, result) in servers.iter().zip(&results) {
            match result {
                Ok(client) => println!("\n{}: {}", server.name, client.base_url()),
                Err(e @ Error::Unreachable { .. }) => println!("\n{e}"),
                Err(e) => println!("\n{}: {e}", server.name),
            }
        }
    }

    ctx.remember(
        servers
            .iter()
            .zip(&results)
            .filter_map(|(s, r)| r.as_ref().ok().map(|c| (s, c))),
    )?;
    Ok(())
}

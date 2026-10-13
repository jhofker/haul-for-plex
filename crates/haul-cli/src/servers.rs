use anyhow::Result;
use futures_util::future::join_all;
use haul_core::Error;

use crate::context::Context;
use crate::table;

pub async fn list(mut ctx: Context, verbose: bool, json: bool) -> Result<()> {
    let servers = ctx.plextv()?.servers().await?;
    if servers.is_empty() {
        if json {
            crate::output::json(&Vec::<()>::new());
        } else {
            println!("No servers found on this Plex account.");
        }
        return Ok(());
    }

    let results = join_all(servers.iter().map(|s| ctx.connect(s))).await;

    if json {
        let entries: Vec<_> = servers
            .iter()
            .zip(&results)
            .map(|(server, result)| {
                let connection = result.as_ref().ok().map(|client| {
                    let r = client.reachable();
                    serde_json::json!({
                        "kind": connection_kind(&r.connection),
                        "uri": r.connection.uri,
                        "latency_ms": r.latency.as_millis() as u64,
                    })
                });
                serde_json::json!({
                    "name": server.name,
                    "id": server.client_identifier,
                    "owned": server.owned,
                    "owner": if server.owned { None } else { server.source_title.clone() },
                    "version": server.product_version,
                    "reachable": connection.is_some(),
                    "connection": connection,
                })
            })
            .collect();
        crate::output::json(&entries);
        return remember(&mut ctx, &servers, &results);
    }

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
                    let kind = connection_kind(&r.connection);
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

    remember(&mut ctx, &servers, &results)
}

fn connection_kind(conn: &haul_core::plextv::Connection) -> &'static str {
    match (conn.relay, conn.local) {
        (true, _) => "relay",
        (false, true) => "local",
        (false, false) => "remote",
    }
}

fn remember(
    ctx: &mut Context,
    servers: &[haul_core::Resource],
    results: &[haul_core::Result<haul_core::ServerClient>],
) -> Result<()> {
    ctx.remember(
        servers
            .iter()
            .zip(results)
            .filter_map(|(s, r)| r.as_ref().ok().map(|c| (s, c))),
    )
}

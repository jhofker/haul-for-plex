//! `haul my-library`: which servers count as your own library.

use anyhow::Result;

use crate::context::Context;

pub async fn run(
    mut ctx: Context,
    servers: &[String],
    owned: bool,
    refresh: bool,
    json: bool,
) -> Result<()> {
    let all = ctx.servers().await?;
    if owned || !servers.is_empty() {
        let mut chosen = Vec::new();
        for query in servers {
            let id = haul_core::find_server(&all, query)?
                .client_identifier
                .clone();
            if !chosen.contains(&id) {
                chosen.push(id);
            }
        }
        ctx.config.my_servers = chosen;
        ctx.store.save(&ctx.config)?;
    }

    let mine: Vec<_> = haul_core::my_servers(&all, &ctx.config)
        .into_iter()
        .cloned()
        .collect();
    if mine.is_empty() {
        if json {
            crate::output::json(&serde_json::json!({"servers": [], "movies": 0, "shows": 0}));
        } else {
            println!(
                "No server counts as your library: you don't own one. \
                 Pick servers with `haul my-library <SERVER>...`."
            );
        }
        return Ok(());
    }

    let lib = ctx
        .my_library(refresh || owned || !servers.is_empty())
        .await?;
    let (movies, shows) = lib.counts();
    if json {
        let names: Vec<_> = mine.iter().map(|s| &s.name).collect();
        crate::output::json(&serde_json::json!({
            "servers": names, "movies": movies, "shows": shows, "skipped": lib.skipped,
        }));
        return Ok(());
    }
    let names: Vec<_> = mine.iter().map(|s| s.name.as_str()).collect();
    let how = if ctx.config.my_servers.is_empty() {
        " (every server you own)"
    } else {
        ""
    };
    println!("Your library: {}{how}", names.join(", "));
    println!("{movies} movies, {shows} shows");
    Ok(())
}

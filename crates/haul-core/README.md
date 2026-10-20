# haul-core

The engine behind [Haul for Plex](https://github.com/jhofker/haul-for-plex):
sign in with a Plex account, find the servers it can reach, browse and search
their libraries, and download files into a clean Plex-style folder layout, with
resume, version choice and size checks.

Most people want the `haul` program instead: `cargo install haul-for-plex`.
Use this crate to build your own tool on the same pieces.

```rust,no_run
use std::time::Duration;

use haul_core::{ClientInfo, ConfigStore, Layout, PlexTvClient, ServerClient, VersionChoice, View};

#[tokio::main]
async fn main() -> haul_core::Result<()> {
    // Sign in: approve the link in a browser (plex.tv handles the password).
    let config = ConfigStore::from_env()?.load_or_init()?;
    let info = ClientInfo::new(config.client_id.clone());
    let tv = PlexTvClient::new(info.clone())?;
    let pin = tv.create_pin().await?;
    println!("Approve at {}", tv.auth_url(&pin));
    let token = tv.wait_for_pin(&pin, Duration::from_secs(2)).await?;

    // Pick a server and look at a library.
    let servers = tv.with_token(token).servers().await?;
    let server = ServerClient::connect(&servers[0], &info, None).await?;
    let (movies, _total) = server.library_page("1", 0, 20, &View::default()).await?;
    let movie = server.metadata(&movies[0].rating_key).await?;

    // Work out where the files go, then download them.
    let layout = Layout { movies_dir: "movies".into(), tv_dir: "tv".into() };
    let plan = server
        .plan_download_version(&movie, &layout, &VersionChoice::First)
        .await?;
    for file in &plan {
        server.download_part(&file.part, &file.dest, false, |_progress| {}).await?;
    }
    Ok(())
}
```

What's in it:

- `plextv`: PIN sign-in, the account's servers
- `server`: connecting to a server by probing its addresses (local, then
  remote, then relay)
- `library`, `listing`: libraries, items, search, filtering and sorting
- `download`: planning files (movies, episodes, seasons, whole shows),
  choosing between versions, and resumable downloads
- `retry`: retrying a download with backoff when the server drops
  (`Error::is_retryable` tells temporary failures from permanent ones)
- `naming`: Plex/Jellyfin-style paths, safe on Windows
- `config`: the sign-in token and settings file

Haul for Plex is an independent project. It is not affiliated with, endorsed
by, or sponsored by Plex, Inc. Plex is a trademark of Plex, Inc.

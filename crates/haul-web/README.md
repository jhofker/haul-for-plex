# haul-web

The web interface of [Haul for Plex](https://github.com/jhofker/haul-for-plex),
as a library: an [axum](https://crates.io/crates/axum) router rendering
server-side HTML with [htmx](https://htmx.org). Sign in with Plex, browse and
search your servers, and queue downloads that are saved on the machine running
it. Downloads can be paused and resumed, are retried when a server drops, and
the queue is saved to `queue.json` beside the config file so it survives a
restart. Single user.

Most people get this through the `haul` program (`haul serve`), installed with
`cargo install haul-for-plex`. Use this crate to embed the interface in your own
program.

```rust,no_run
use haul_core::{ConfigStore, Layout};
use haul_web::Settings;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = Settings {
        store: ConfigStore::from_env()?,
        layout: Layout { movies_dir: "movies".into(), tv_dir: "tv".into() },
        password: Some("something long".into()), // HTTP basic auth, any username
        parallel: 2,
        plextv_url: None,
        retry: Settings::DEFAULT_RETRY, // waits when a server drops
    };
    haul_web::serve("127.0.0.1:8080".parse()?, settings).await
}
```

To mount it inside an existing axum app, use `haul_web::router(settings)`
instead of `serve`. `serve` also stops downloads cleanly on Ctrl-C or SIGTERM
so they resume on the next start.

If you listen beyond localhost, set a password and put the port behind a
reverse proxy that provides HTTPS.

Haul for Plex is an independent project. It is not affiliated with, endorsed
by, or sponsored by Plex, Inc. Plex is a trademark of Plex, Inc.

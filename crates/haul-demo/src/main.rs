//! `haul-demo`: run the real Haul web interface against fake Plex servers
//! holding made-up media. Handy for screenshots; touches nothing real.
//!
//!     cargo run -p haul-demo
//!
//! Downloads write zero-filled files into a temporary folder that is removed
//! when you press Ctrl-C.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use haul_core::{Auth, ConfigStore, Layout};
use haul_web::Settings;

#[derive(Parser)]
#[command(about = "Haul for Plex web interface against fake servers and made-up media")]
struct Args {
    /// Address for the web interface.
    #[arg(long, default_value = "127.0.0.1:8080")]
    bind: SocketAddr,
    /// How fast fake downloads stream, in MB/s.
    #[arg(long, default_value_t = 40.0)]
    speed: f64,
    /// Start with an empty downloads page instead of a few queued items.
    #[arg(long)]
    no_seed: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let demo = haul_demo::start(args.speed).await?;

    let dir = tempfile::tempdir()?;
    let store = ConfigStore::in_dir(dir.path().join("config"));
    let mut config = store.load_or_init()?;
    config.auth = Some(Auth {
        token: haul_demo::TOKEN.into(),
        username: haul_demo::USERNAME.into(),
    });
    store.save(&config)?;

    std::env::set_current_dir(dir.path())?;
    let settings = Settings {
        store,
        // Relative, and the process works from inside the temp folder, so the
        // Downloads page shows tidy `downloads/...` paths in screenshots.
        layout: Layout {
            movies_dir: "downloads/Movies".into(),
            tv_dir: "downloads/TV Shows".into(),
        },
        password: None,
        parallel: 2,
        plextv_url: Some(demo.plextv_url.clone()),
    };

    println!(
        "Demo servers: {}",
        demo.servers
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "Fake plex.tv: {} (set HAUL_PLEX_TV_URL to this, and sign in, to try the CLI)",
        demo.plextv_url
    );
    println!(
        "Downloads go to {}/downloads (removed on exit)",
        dir.path().display()
    );
    if !args.no_seed {
        let servers = demo.servers.clone();
        let bind = args.bind;
        tokio::spawn(async move {
            if let Err(e) = seed(bind, &servers).await {
                eprintln!("could not queue demo downloads: {e:#}");
            }
        });
    }
    haul_web::serve(args.bind, settings).await
}

/// Queue a handful of downloads so the Downloads page has something on it.
async fn seed(
    bind: SocketAddr,
    servers: &[std::sync::Arc<haul_demo::catalog::Server>],
) -> Result<()> {
    let base = format!("http://{bind}");
    let http = reqwest::Client::new();
    for _ in 0..50 {
        if http.get(format!("{base}/login")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let atlas = &servers[0];
    let lighthouse = &servers[1];
    // Episodes of a season, by number (1-based).
    let episode = |show: &str, season: usize, n: usize| -> Result<&str> {
        let season = atlas.season(show, season).context("season")?;
        atlas.items[season]
            .children
            .get(n - 1)
            .map(String::as_str)
            .context("episode")
    };
    // Queued in this order and run two at a time, so the page ends up with a
    // refused download, two finished, two in progress and one waiting.
    let queue: Vec<(&str, &str, Option<&str>)> = vec![
        (
            &lighthouse.id,
            lighthouse.find("Tidewater").context("Tidewater")?,
            None,
        ),
        (&atlas.id, episode("Foxglove", 1, 1)?, None),
        (&atlas.id, episode("Foxglove", 1, 2)?, None),
        (
            &atlas.id,
            atlas.find("Orbit Seven").context("Orbit Seven")?,
            Some("2"),
        ),
        (&atlas.id, episode("Harbor Lights", 1, 1)?, None),
        (&atlas.id, episode("Harbor Lights", 1, 2)?, None),
    ];
    for (server, key, version) in queue {
        let mut form = Vec::new();
        if let Some(v) = version {
            form.push(("version", v));
        }
        http.post(format!("{base}/servers/{server}/items/{key}/download"))
            .header("Origin", &base)
            .form(&form)
            .send()
            .await?
            .error_for_status()?;
        // Keep the order stable on the Downloads page.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Ok(())
}

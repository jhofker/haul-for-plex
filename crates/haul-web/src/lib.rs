#![doc = include_str!("../README.md")]

mod downloads;
mod guard;
mod listing;
mod routes;
mod state;
mod views;

use std::net::SocketAddr;

pub use routes::router;
pub use state::Settings;

/// Serve the web interface on `bind` until interrupted.
pub async fn serve(bind: SocketAddr, settings: Settings) -> anyhow::Result<()> {
    let open = settings.password.is_none() && !bind.ip().is_loopback();
    let app = router(settings);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    println!(
        "Haul for Plex web interface listening on http://{}",
        listener.local_addr()?
    );
    if open {
        eprintln!(
            "warning: listening beyond localhost with no password; \
             anyone who can reach this port can use your Plex account. \
             Set a password with --password or HAUL_WEB_PASSWORD."
        );
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

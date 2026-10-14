//! Fake Plex servers with made-up media, so the web interface can be tried
//! out and screenshotted without a real account. Not shipped with Haul:
//! this crate is `publish = false` and nothing else depends on it.

pub mod art;
pub mod catalog;
pub mod plextv;
pub mod pms;

use std::sync::Arc;

use tokio::net::TcpListener;

pub const USERNAME: &str = "jordan";
pub const TOKEN: &str = "demo-account-token";

/// A running fake plex.tv and Plex servers, all on local ports.
pub struct Demo {
    /// Base URL for [`haul_web::Settings::plextv_url`].
    pub plextv_url: String,
    pub servers: Vec<Arc<catalog::Server>>,
}

async fn spawn(app: axum::Router) -> anyhow::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(port)
}

/// Start everything. `speed_mb_s` is how fast downloads stream.
pub async fn start(speed_mb_s: f64) -> anyhow::Result<Demo> {
    let mut entries = Vec::new();
    let mut servers = Vec::new();
    for server in catalog::build() {
        let server = Arc::new(server);
        let port = spawn(pms::router(server.clone(), speed_mb_s)).await?;
        entries.push(plextv::ServerEntry {
            name: server.name.clone(),
            id: server.id.clone(),
            token: server.token.clone(),
            owned: server.owned,
            owner: server.owner.clone(),
            uri: format!("http://127.0.0.1:{port}"),
            port,
        });
        servers.push(server);
    }
    let tv = Arc::new(plextv::PlexTv {
        username: USERNAME.into(),
        token: TOKEN.into(),
        servers: entries,
    });
    let port = spawn(plextv::router(tv)).await?;
    Ok(Demo {
        plextv_url: format!("http://127.0.0.1:{port}/"),
        servers,
    })
}

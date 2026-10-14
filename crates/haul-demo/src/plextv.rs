//! A fake plex.tv: sign-in that approves itself, and the list of servers.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, State};
use axum::response::Json;
use axum::routing::{get, post};
use serde_json::{Value, json};

pub struct ServerEntry {
    pub name: String,
    pub id: String,
    pub token: String,
    pub owned: bool,
    pub owner: Option<String>,
    pub uri: String,
    pub port: u16,
}

pub struct PlexTv {
    pub username: String,
    pub token: String,
    pub servers: Vec<ServerEntry>,
}

pub fn router(state: Arc<PlexTv>) -> Router {
    Router::new()
        .route("/api/v2/pins", post(create_pin))
        .route("/api/v2/pins/{id}", get(check_pin))
        .route("/api/v2/user", get(user))
        .route("/api/v2/resources", get(resources))
        .with_state(state)
}

async fn create_pin() -> Json<Value> {
    Json(json!({"id": 1, "code": "DEMO", "authToken": null, "expiresIn": 900}))
}

async fn check_pin(State(s): State<Arc<PlexTv>>, Path(id): Path<u64>) -> Json<Value> {
    // Approved straight away.
    Json(json!({"id": id, "code": "DEMO", "authToken": s.token, "expiresIn": 900}))
}

async fn user(State(s): State<Arc<PlexTv>>) -> Json<Value> {
    Json(json!({
        "id": 1, "uuid": "0000-demo", "username": s.username,
        "title": s.username, "email": null
    }))
}

async fn resources(State(s): State<Arc<PlexTv>>) -> Json<Value> {
    let list: Vec<Value> = s
        .servers
        .iter()
        .map(|v| {
            json!({
                "name": v.name, "product": "Plex Media Server", "productVersion": "1.41.0",
                "platform": "Linux", "clientIdentifier": v.id, "provides": "server",
                "owned": v.owned, "sourceTitle": v.owner, "presence": true,
                "accessToken": v.token,
                "connections": [{
                    "protocol": "http", "address": "127.0.0.1", "port": v.port,
                    "uri": v.uri, "local": true, "relay": false, "IPv6": false
                }]
            })
        })
        .collect();
    Json(json!(list))
}

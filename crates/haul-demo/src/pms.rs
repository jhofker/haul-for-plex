//! A fake Plex Media Server: just the endpoints Haul uses.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::stream;
use serde_json::{Value, json};

use crate::art;
use crate::catalog::{Item, Server};

#[derive(Clone)]
struct Pms {
    server: Arc<Server>,
    /// Download speed in bytes per second.
    speed: f64,
}

pub fn router(server: Arc<Server>, speed_mb_s: f64) -> Router {
    let state = Pms {
        server,
        speed: speed_mb_s.max(0.1) * 1_000_000.0,
    };
    // Plex really does serve images from `/photo/:/transcode`.
    Router::new()
        .without_v07_checks()
        .route("/identity", get(identity))
        .route("/library/sections", get(sections))
        .route("/library/sections/{id}/all", get(all))
        .route("/library/metadata/{key}", get(metadata))
        .route("/library/metadata/{key}/children", get(children))
        .route("/library/metadata/{key}/allLeaves", get(all_leaves))
        .route("/library/metadata/{key}/thumb", get(thumb))
        .route("/photo/:/transcode", get(transcode))
        .route("/hubs/search", get(search))
        .route("/library/parts/{id}/{ts}/{file}", get(part))
        .with_state(state)
}

fn container(body: Value) -> Response {
    axum::Json(json!({ "MediaContainer": body })).into_response()
}

fn svg(markup: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        markup,
    )
        .into_response()
}

async fn identity(State(s): State<Pms>) -> Response {
    container(json!({ "machineIdentifier": s.server.id, "version": "1.41.0" }))
}

async fn sections(State(s): State<Pms>) -> Response {
    let dirs: Vec<Value> = s
        .server
        .libraries
        .iter()
        .map(|l| json!({"key": l.key, "title": l.title, "type": l.kind}))
        .collect();
    container(json!({ "size": dirs.len(), "Directory": dirs }))
}

fn num(headers: &HeaderMap, q: &HashMap<String, String>, name: &str) -> Option<usize> {
    q.get(name)
        .map(String::as_str)
        .or_else(|| {
            headers
                .get(format!("x-plex-container-{name}"))
                .and_then(|v| v.to_str().ok())
        })
        .and_then(|v| v.parse().ok())
}

async fn all(
    State(s): State<Pms>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let Some(lib) = s.server.libraries.iter().find(|l| l.key == id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut items: Vec<&Item> = lib
        .items
        .iter()
        .filter_map(|k| s.server.items.get(k))
        .collect();
    // `type=4` lists every episode in a TV library instead of its shows.
    if q.get("type").map(String::as_str) == Some("4") {
        items = items
            .iter()
            .flat_map(|show| &show.children)
            .filter_map(|season| s.server.items.get(season))
            .flat_map(|season| &season.children)
            .filter_map(|ep| s.server.items.get(ep))
            .collect();
    }
    if let Some(title) = q.get("title") {
        let t = title.to_lowercase();
        items.retain(|i| i.title.to_lowercase().contains(&t));
    }
    if let Some(year) = q.get("year").and_then(|y| y.parse::<u32>().ok()) {
        items.retain(|i| i.year == Some(year));
    }
    let (field, dir) = q
        .get("sort")
        .map(|s| s.split_once(':').unwrap_or((s, "asc")))
        .unwrap_or(("titleSort", "asc"));
    items.sort_by(|a, b| {
        let o = match field {
            "year" => a.year.cmp(&b.year),
            "addedAt" => a.added.cmp(&b.added),
            "originallyAvailableAt" => a.released.cmp(&b.released),
            "audienceRating" => a.audience.total_cmp(&b.audience),
            _ => a
                .sort_title
                .to_lowercase()
                .cmp(&b.sort_title.to_lowercase()),
        };
        if dir == "desc" { o.reverse() } else { o }
    });
    let total = items.len();
    let start = num(&headers, &q, "start").unwrap_or(0).min(total);
    let size = num(&headers, &q, "size").unwrap_or(total).max(1);
    let page: Vec<&Value> = items
        .iter()
        .skip(start)
        .take(size)
        .map(|i| &i.json)
        .collect();
    container(json!({
        "size": page.len(), "totalSize": total, "offset": start, "Metadata": page
    }))
}

async fn metadata(State(s): State<Pms>, Path(key): Path<String>) -> Response {
    match s.server.items.get(&key) {
        Some(item) => container(json!({ "size": 1, "Metadata": [item.json] })),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn children(State(s): State<Pms>, Path(key): Path<String>) -> Response {
    let Some(item) = s.server.items.get(&key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let kids: Vec<&Value> = item
        .children
        .iter()
        .filter_map(|k| s.server.items.get(k))
        .map(|i| &i.json)
        .collect();
    container(json!({ "size": kids.len(), "Metadata": kids }))
}

async fn all_leaves(State(s): State<Pms>, Path(key): Path<String>) -> Response {
    let Some(show) = s.server.items.get(&key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut eps = Vec::new();
    for season in &show.children {
        if let Some(season) = s.server.items.get(season) {
            eps.extend(
                season
                    .children
                    .iter()
                    .filter_map(|k| s.server.items.get(k))
                    .map(|i| &i.json),
            );
        }
    }
    container(json!({ "size": eps.len(), "Metadata": eps }))
}

fn cover_for(server: &Server, key: &str) -> Option<String> {
    let item = server.items.get(key)?;
    Some(match item.kind {
        "episode" => {
            let season = item.json["parentIndex"].as_u64().unwrap_or(0);
            let ep = item.json["index"].as_u64().unwrap_or(0);
            art::cover(&item.title, &format!("S{season:02}E{ep:02}"), true)
        }
        "season" => art::cover(
            item.json["parentTitle"].as_str().unwrap_or(""),
            &item.title,
            false,
        ),
        _ => art::cover(
            &item.title,
            &item.year.map(|y| y.to_string()).unwrap_or_default(),
            false,
        ),
    })
}

async fn thumb(State(s): State<Pms>, Path(key): Path<String>) -> Response {
    match cover_for(&s.server, &key) {
        Some(markup) => svg(markup),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Plex's image resizer: `url=/library/metadata/{key}/thumb`.
async fn transcode(State(s): State<Pms>, Query(q): Query<HashMap<String, String>>) -> Response {
    let key = q
        .get("url")
        .and_then(|u| u.split('/').nth(3))
        .map(String::from)
        .unwrap_or_default();
    match cover_for(&s.server, &key) {
        Some(markup) => svg(markup),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Plex search is loose; return anything sharing a word and let Haul narrow it.
async fn search(State(s): State<Pms>, Query(q): Query<HashMap<String, String>>) -> Response {
    let words: Vec<String> = q
        .get("query")
        .map(|v| v.split_whitespace().map(str::to_lowercase).collect())
        .unwrap_or_default();
    let limit: usize = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(10);
    let mut hubs = Vec::new();
    for kind in ["movie", "show", "season", "episode"] {
        let hits: Vec<&Value> = s
            .server
            .items
            .values()
            .filter(|i| i.kind == kind)
            .filter(|i| {
                let hay = format!(
                    "{} {}",
                    i.title,
                    i.show
                        .as_ref()
                        .and_then(|k| s.server.items.get(k))
                        .map(|sh| sh.title.as_str())
                        .unwrap_or("")
                )
                .to_lowercase();
                words.iter().any(|w| hay.contains(w.as_str()))
            })
            .take(limit)
            .map(|i| &i.json)
            .collect();
        if !hits.is_empty() {
            hubs.push(json!({"type": kind, "Metadata": hits}));
        }
    }
    container(json!({ "size": hubs.len(), "Hub": hubs }))
}

/// A file made of zeros, at the configured speed, with `Range` support so
/// resuming works the way it does against a real server.
async fn part(
    State(s): State<Pms>,
    Path((id, _ts, _file)): Path<(u64, String, String)>,
    headers: HeaderMap,
) -> Response {
    let Some(info) = s.server.parts.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if s.server.forbidden.contains(&id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let total = info.size;
    let start = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| v.split('-').next())
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|s| *s < total);
    let from = start.unwrap_or(0);
    let speed = s.speed;
    const CHUNK: u64 = 1 << 20;
    let body = Body::from_stream(stream::unfold(from, move |pos| async move {
        if pos >= total {
            return None;
        }
        let n = CHUNK.min(total - pos);
        tokio::time::sleep(Duration::from_secs_f64(n as f64 / speed)).await;
        Some((Ok::<_, Infallible>(vec![0u8; n as usize]), pos + n))
    }));
    let mut resp = Response::new(body);
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("video/x-matroska"),
    );
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(total - from));
    if start.is_some() {
        *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
        resp.headers_mut().insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {from}-{}/{total}", total - 1)).unwrap(),
        );
    }
    resp
}

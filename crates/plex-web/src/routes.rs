use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use futures_util::future::join_all;
use maud::Markup;
use plex_core::{Error, Metadata};
use serde::Deserialize;

use crate::guard::guard;
use crate::state::{AppState, Settings};
use crate::views::{self, ItemPage, PAGE_SIZE, SearchResults};

pub fn router(settings: Settings) -> Router {
    let state = AppState::new(settings);
    Router::new()
        .route("/", get(|| async { Redirect::to("/servers") }))
        .route("/login", get(login_page).post(login_start))
        .route("/login/status", get(login_status))
        .route("/logout", post(logout))
        .route("/servers", get(servers))
        .route("/servers/{id}", get(libraries))
        .route("/servers/{id}/status", get(server_status))
        .route("/servers/{id}/libraries/{key}", get(library))
        .route("/servers/{id}/items/{key}", get(item))
        .route("/servers/{id}/items/{key}/download", post(download))
        .route("/search", get(search))
        .route("/downloads", get(downloads))
        .route("/downloads/list", get(downloads_list))
        .route("/downloads/clear", post(downloads_clear))
        .route("/downloads/{id}/cancel", post(downloads_cancel))
        .route("/static/htmx.min.js", get(htmx))
        .route("/static/app.css", get(css))
        .route("/static/theme.js", get(theme_js))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

// ---- errors ---------------------------------------------------------------

pub enum AppError {
    Plex(Error),
}

impl From<Error> for AppError {
    fn from(e: Error) -> Self {
        AppError::Plex(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, heading, message) = match self {
            AppError::Plex(Error::NotLoggedIn) => return Redirect::to("/login").into_response(),
            // plex.tv or the server rejected our token: sign in again.
            AppError::Plex(Error::Status { status, .. }) if status == StatusCode::UNAUTHORIZED => {
                return Redirect::to("/login").into_response();
            }
            AppError::Plex(e) => {
                let status = match e {
                    Error::ItemNotFound(_) | Error::ServerNotFound(_) => StatusCode::NOT_FOUND,
                    Error::Unreachable { .. } | Error::Http(_) | Error::Status { .. } => {
                        StatusCode::BAD_GATEWAY
                    }
                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                };
                let heading = match status {
                    StatusCode::NOT_FOUND => "Not found",
                    StatusCode::BAD_GATEWAY => "Plex can't be reached",
                    _ => "Something went wrong",
                };
                (status, heading, e.to_string())
            }
        };
        (status, views::error_page(None, heading, &message)).into_response()
    }
}

type Page = Result<Markup, AppError>;

/// The signed-in username, or a redirect to the login page.
fn user(state: &AppState) -> Result<String, AppError> {
    state
        .config()?
        .auth
        .map(|a| a.username)
        .ok_or(Error::NotLoggedIn.into())
}

// ---- login ----------------------------------------------------------------

async fn login_page(State(state): State<AppState>) -> Response {
    if state.username().is_some() {
        return Redirect::to("/servers").into_response();
    }
    views::login_page(None).into_response()
}

async fn login_start(State(state): State<AppState>) -> Page {
    let plextv = state.plextv_anonymous()?;
    let pin = plextv.create_pin().await?;
    let url = plextv.auth_url(&pin);
    state.set_pending_pin(Some(pin));
    Ok(views::login_waiting_page(&url))
}

async fn login_status(State(state): State<AppState>) -> Result<Response, AppError> {
    let Some(pin) = state.pending_pin() else {
        return Ok(views::login_expired().into_response());
    };
    let plextv = state.plextv_anonymous()?;
    let pin = match plextv.check_pin(pin.id).await {
        Ok(pin) => pin,
        Err(Error::Status { status, .. }) if status == StatusCode::NOT_FOUND => {
            state.set_pending_pin(None);
            return Ok(views::login_expired().into_response());
        }
        Err(e) => return Err(e.into()),
    };

    let Some(token) = pin.auth_token.clone().filter(|t| !t.is_empty()) else {
        return Ok(views::login_waiting(&plextv.auth_url(&pin), None).into_response());
    };

    let user = plextv.with_token(&token).user().await?;
    let mut config = state.config()?;
    config.auth = Some(plex_core::Auth {
        token,
        username: user.username,
    });
    state.settings().store.save(&config)?;
    state.set_pending_pin(None);
    state.invalidate();
    Ok(([("HX-Redirect", "/servers")], "").into_response())
}

async fn logout(State(state): State<AppState>) -> Result<Redirect, AppError> {
    let mut config = state.config()?;
    config.auth = None;
    state.settings().store.save(&config)?;
    state.invalidate();
    Ok(Redirect::to("/login"))
}

// ---- browsing -------------------------------------------------------------

async fn servers(State(state): State<AppState>) -> Page {
    let user = user(&state)?;
    let servers = state.servers().await?;
    Ok(views::servers_page(&user, &servers))
}

async fn server_status(State(state): State<AppState>, Path(id): Path<String>) -> Markup {
    match state.server(&id).await {
        Ok((_, client)) => {
            let r = client.reachable();
            views::server_status(Ok((&r.connection, r.latency.as_millis())))
        }
        Err(e) => views::server_status(Err(e.to_string())),
    }
}

async fn libraries(State(state): State<AppState>, Path(id): Path<String>) -> Page {
    let user = user(&state)?;
    let (server, client) = state.server(&id).await?;
    let libraries = client.libraries().await?;
    Ok(views::libraries_page(&user, &server, &libraries))
}

#[derive(Deserialize)]
struct PageQuery {
    page: Option<usize>,
}

async fn library(
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
    Query(query): Query<PageQuery>,
) -> Page {
    let user = user(&state)?;
    let (server, client) = state.server(&id).await?;
    let page = query.page.unwrap_or(1).max(1);
    let (items, total) = client
        .library_page(&key, (page - 1) * PAGE_SIZE, PAGE_SIZE)
        .await?;
    let title = client
        .libraries()
        .await?
        .into_iter()
        .find(|l| l.key == key)
        .map(|l| l.title)
        .ok_or_else(|| Error::ItemNotFound(key.clone()))?;
    Ok(views::library_page(
        &user, &server, &key, &title, &items, page, total,
    ))
}

async fn item(State(state): State<AppState>, Path((id, key)): Path<(String, String)>) -> Page {
    let user = user(&state)?;
    let (server, client) = state.server(&id).await?;
    let item = client.metadata(&key).await?;

    let item_link = |key: &Option<String>, title: &Option<String>| match (key, title) {
        (Some(k), Some(t)) => Some((format!("/servers/{id}/items/{k}"), t.clone())),
        _ => None,
    };
    let mut crumbs = Vec::new();
    if let (Some(lib), Some(title)) = (item.library_section_id, &item.library_section_title) {
        crumbs.push((format!("/servers/{id}/libraries/{lib}"), title.clone()));
    }
    match item.kind.as_str() {
        "episode" => {
            crumbs.extend(item_link(
                &item.grandparent_rating_key,
                &item.grandparent_title,
            ));
            crumbs.extend(item_link(&item.parent_rating_key, &item.parent_title));
        }
        "season" => crumbs.extend(item_link(&item.parent_rating_key, &item.parent_title)),
        _ => {}
    }

    let children = match item.kind.as_str() {
        "show" | "season" => client.children(&item.rating_key).await?,
        _ => Vec::new(),
    };
    // Not every item can be downloaded (e.g. music); then there's no button.
    let plan = client
        .plan_download(&item, &state.settings().layout)
        .await
        .unwrap_or_default();

    Ok(views::item_page(&ItemPage {
        user: &user,
        server: &server,
        item: &item,
        crumbs,
        children: &children,
        plan: &plan,
    }))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
}

async fn search(State(state): State<AppState>, Query(query): Query<SearchQuery>) -> Page {
    let user = user(&state)?;
    let q = query.q.unwrap_or_default();
    let q = q.trim();
    if q.is_empty() {
        return Ok(views::search_page(&user, "", None));
    }

    let servers = state.servers().await?;
    let outcomes = join_all(servers.iter().map(|s| {
        let state = state.clone();
        let id = s.client_identifier.clone();
        async move {
            let (resource, client) = state.server(&id).await?;
            let items: Vec<Metadata> = client.search(q, 20).await?;
            Ok::<_, Error>((resource, items))
        }
    }))
    .await;

    let mut found = Vec::new();
    let mut skipped = Vec::new();
    for (server, outcome) in servers.iter().zip(outcomes) {
        match outcome {
            Ok(entry) => found.push(entry),
            Err(_) => skipped.push(server.name.clone()),
        }
    }
    Ok(views::search_page(
        &user,
        q,
        Some(SearchResults {
            found: &found,
            skipped: &skipped,
        }),
    ))
}

// ---- downloads ------------------------------------------------------------

async fn download(
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
) -> Result<Redirect, AppError> {
    user(&state)?;
    let (server, client) = state.server(&id).await?;
    let item = client.metadata(&key).await?;
    let plan = client
        .plan_download(&item, &state.settings().layout)
        .await?;
    state.downloads().enqueue(&client, &server.name, plan);
    Ok(Redirect::to("/downloads"))
}

async fn downloads(State(state): State<AppState>) -> Page {
    let user = user(&state)?;
    Ok(views::downloads_page(&user, &state.downloads().snapshot()))
}

async fn downloads_list(State(state): State<AppState>) -> Page {
    user(&state)?;
    Ok(views::downloads_list(&state.downloads().snapshot()))
}

async fn downloads_clear(State(state): State<AppState>) -> Result<Redirect, AppError> {
    user(&state)?;
    state.downloads().clear_finished();
    Ok(Redirect::to("/downloads"))
}

async fn downloads_cancel(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Redirect, AppError> {
    user(&state)?;
    state.downloads().cancel(id);
    Ok(Redirect::to("/downloads"))
}

// ---- assets ---------------------------------------------------------------

async fn htmx() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_str!("assets/htmx.min.js"),
    )
}

async fn theme_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        include_str!("assets/theme.js"),
    )
}

async fn css() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        include_str!("assets/app.css"),
    )
}

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use futures_util::future::join_all;
use haul_core::{Error, Metadata, Resource, VersionChoice};
use maud::Markup;
use serde::Deserialize;

use crate::guard::guard;
use crate::listing::ListParams;
use crate::state::{AppState, Settings};
use crate::views::{self, ItemPage, LibraryPage, PAGE_SIZE};

pub fn router(settings: Settings) -> Router {
    router_for(AppState::new(settings))
}

pub(crate) fn router_for(state: AppState) -> Router {
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
        .route("/servers/{id}/items/{key}/cover", get(cover))
        .route("/search", get(search))
        .route("/downloads", get(downloads))
        .route("/downloads/list", get(downloads_list))
        .route("/downloads/clear", post(downloads_clear))
        .route("/downloads/pause", post(downloads_pause_all))
        .route("/downloads/resume", post(downloads_resume_all))
        .route("/downloads/{id}/cancel", post(downloads_cancel))
        .route("/downloads/{id}/pause", post(downloads_pause))
        .route("/downloads/{id}/resume", post(downloads_resume))
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
    config.auth = Some(haul_core::Auth {
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
    Query(params): Query<ListParams>,
) -> Page {
    let user = user(&state)?;
    let (server, client) = state.server(&id).await?;
    let page = query.page.unwrap_or(1).max(1);
    let (items, total) = client
        .library_page(&key, (page - 1) * PAGE_SIZE, PAGE_SIZE, &params.view())
        .await?;
    let title = client
        .libraries()
        .await?
        .into_iter()
        .find(|l| l.key == key)
        .map(|l| l.title)
        .ok_or_else(|| Error::ItemNotFound(key.clone()))?;
    Ok(views::library_page(&LibraryPage {
        user: &user,
        server: &server,
        library_key: &key,
        library_title: &title,
        items: &items,
        page,
        total,
        params: &params,
    }))
}

async fn item(
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
    Query(mut params): Query<ListParams>,
) -> Page {
    // Seasons and episodes offer a title filter and sorting only.
    params.year.clear();
    params.kind.clear();
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

    let mut children = match item.kind.as_str() {
        "show" | "season" => client.children(&item.rating_key).await?,
        _ => Vec::new(),
    };
    let child_count = children.len();
    params.view().apply(&mut children);
    // Not every item can be downloaded (e.g. music); then there's no button.
    let layout = &state.settings().layout;
    let plan = client
        .plan_download(&item, layout)
        .await
        .unwrap_or_default();
    // A movie or episode with several versions gets a button for each.
    let mut version_plans = Vec::new();
    if item.is_playable() && item.media.len() > 1 {
        for (i, media) in item.media.iter().enumerate() {
            if media.parts.is_empty() {
                continue;
            }
            let choice = VersionChoice::Index(i + 1);
            if let Ok(files) = client.plan_download_version(&item, layout, &choice).await {
                version_plans.push((i + 1, files));
            }
        }
    }

    Ok(views::item_page(&ItemPage {
        user: &user,
        server: &server,
        item: &item,
        crumbs,
        children: &children,
        child_count,
        params: &params,
        plan: &plan,
        version_plans: &version_plans,
    }))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    /// Restrict to one server (client identifier).
    server: Option<String>,
    /// Restrict to one library of that server (section key).
    library: Option<String>,
}

async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
    Query(mut params): Query<ListParams>,
) -> Page {
    // Search results offer type and sort menus, not a second text filter.
    params.title.clear();
    params.year.clear();
    let user = user(&state)?;
    let q = query.q.unwrap_or_default();
    let q = q.trim();
    let server_id = query.server.filter(|s| !s.is_empty());
    // A library only makes sense within a server.
    let library = query
        .library
        .filter(|l| !l.is_empty() && server_id.is_some());

    let scoped = match &server_id {
        Some(id) => Some(state.server(id).await?),
        None => None,
    };
    let library_title = match (&scoped, &library) {
        (Some((_, client)), Some(key)) => client
            .libraries()
            .await?
            .into_iter()
            .find(|l| &l.key == key)
            .map(|l| l.title),
        _ => None,
    };
    let scope = views::SearchScope {
        server: server_id.as_deref(),
        library: library.as_deref(),
    };
    let page = |results| {
        views::search_page(&views::SearchPage {
            user: &user,
            query: q,
            scope,
            server: scoped.as_ref().map(|(r, _)| r),
            library_title: library_title.as_deref(),
            params: &params,
            results,
        })
    };
    if q.is_empty() {
        return Ok(page(None));
    }

    let outcomes: Vec<(Resource, Result<Vec<Metadata>, Error>)> = match &scoped {
        Some((resource, client)) => {
            let items = search_one(client, q, library.as_deref()).await;
            vec![(resource.clone(), items)]
        }
        None => {
            let servers = state.servers().await?;
            join_all(servers.into_iter().map(|resource| {
                let state = state.clone();
                async move {
                    let items = match state.server(&resource.client_identifier).await {
                        Ok((_, client)) => search_one(&client, q, None).await,
                        Err(e) => Err(e),
                    };
                    (resource, items)
                }
            }))
            .await
        }
    };

    let view = params.view();
    let mut found = Vec::new();
    let mut skipped = Vec::new();
    for (resource, outcome) in outcomes {
        match outcome {
            Ok(mut items) => {
                view.apply(&mut items);
                found.push((resource, items));
            }
            // Searching one chosen server should say why it failed.
            Err(e) if scoped.is_some() => return Err(e.into()),
            Err(_) => skipped.push(resource.name),
        }
    }
    Ok(page(Some(views::SearchResults {
        found: &found,
        skipped: &skipped,
    })))
}

/// Search one server, giving up after 10 seconds so a dead server can't stall the page.
async fn search_one(
    client: &haul_core::ServerClient,
    q: &str,
    library: Option<&str>,
) -> Result<Vec<Metadata>, Error> {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.search_in(q, 20, library),
    )
    .await
    .map_err(|_| Error::Unreachable {
        server: client.name().to_string(),
        attempts: vec![("search".into(), "timed out".into())],
    })?
}

#[derive(Deserialize)]
struct CoverQuery {
    #[serde(default)]
    wide: bool,
}

/// Cover art, proxied so the browser needn't reach the Plex server or see its token.
async fn cover(
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
    Query(query): Query<CoverQuery>,
) -> Result<Response, AppError> {
    user(&state)?;
    let (_, client) = state.server(&id).await?;
    let (w, h) = if query.wide { (400, 225) } else { (300, 450) };
    match client.thumbnail(&key, w, h).await {
        Ok((bytes, kind)) => Ok((
            [
                (header::CONTENT_TYPE, kind),
                (header::CACHE_CONTROL, "private, max-age=86400".into()),
            ],
            bytes,
        )
            .into_response()),
        Err(_) => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

// ---- downloads ------------------------------------------------------------

async fn download(
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<Response, AppError> {
    user(&state)?;
    // `version` is `first`, a version number, or a resolution; the body may be empty.
    let version = url::form_urlencoded::parse(&body)
        .find(|(k, _)| k == "version")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default();
    let version: VersionChoice = match version.parse() {
        Ok(v) => v,
        Err(message) => {
            return Ok((
                StatusCode::BAD_REQUEST,
                views::error_page(None, "Bad request", &message),
            )
                .into_response());
        }
    };
    let (server, client) = state.server(&id).await?;
    let item = client.metadata(&key).await?;
    let plan = client
        .plan_download_version(&item, &state.settings().layout, &version)
        .await?;
    state.downloads().enqueue(
        &server.client_identifier,
        &server.name,
        &key,
        &version.to_string(),
        plan,
    );
    Ok(Redirect::to("/downloads").into_response())
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

async fn downloads_pause(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Redirect, AppError> {
    user(&state)?;
    state.downloads().pause(id);
    Ok(Redirect::to("/downloads"))
}

async fn downloads_resume(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Redirect, AppError> {
    user(&state)?;
    state.downloads().resume(id);
    Ok(Redirect::to("/downloads"))
}

/// The `server` form field, if given: limits a bulk action to one server.
fn server_field(body: &[u8]) -> Option<String> {
    url::form_urlencoded::parse(body)
        .find(|(k, _)| k == "server")
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty())
}

async fn downloads_pause_all(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Redirect, AppError> {
    user(&state)?;
    state.downloads().pause_all(server_field(&body).as_deref());
    Ok(Redirect::to("/downloads"))
}

async fn downloads_resume_all(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Redirect, AppError> {
    user(&state)?;
    state.downloads().resume_all(server_field(&body).as_deref());
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

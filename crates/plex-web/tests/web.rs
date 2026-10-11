use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use plex_core::{Auth, ConfigStore, Layout};
use plex_web::Settings;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SERVER_ID: &str = "srv-1";
const FILE: &[u8] = b"pretend this is a video file";

struct Env {
    app: Router,
    plextv: MockServer,
    store: ConfigStore,
    movies: PathBuf,
    _dir: tempfile::TempDir,
}

fn container(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "MediaContainer": body }))
}

async fn mock_get(server: &MockServer, p: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(response)
        .mount(server)
        .await;
}

/// A signed-in app (unless `logged_in` is false) with a fake plex.tv and one
/// fake Plex server holding a movie and a show.
async fn env(logged_in: bool, password: Option<&str>) -> Env {
    let pms = MockServer::start().await;
    mock_get(
        &pms,
        "/identity",
        container(json!({"machineIdentifier": SERVER_ID})),
    )
    .await;
    mock_get(
        &pms,
        "/library/sections",
        container(json!({"Directory": [{"key": "1", "title": "Movies", "type": "movie"}]})),
    )
    .await;
    // Only answers when the search is limited to library 1.
    Mock::given(method("GET"))
        .and(path("/hubs/search"))
        .and(wiremock::matchers::query_param("sectionId", "1"))
        .respond_with(container(json!({"Hub": [{"type": "movie", "Metadata": [
            {"ratingKey": "10", "type": "movie", "title": "Some Movie", "year": 1999},
            {"ratingKey": "11", "type": "episode", "title": "Pilot", "index": 1,
             "parentIndex": 2, "grandparentTitle": "Some Movie Show",
             "grandparentRatingKey": "20"},
        ]}]})))
        .mount(&pms)
        .await;
    mock_get(
        &pms,
        "/photo/:/transcode",
        ResponseTemplate::new(200)
            .insert_header("content-type", "image/jpeg")
            .set_body_bytes(b"fake-jpeg".to_vec()),
    )
    .await;
    mock_get(
        &pms,
        "/library/sections/1/all",
        container(json!({"totalSize": 250, "Metadata": [
            {"ratingKey": "10", "type": "movie", "title": "<script>alert(1)</script>", "year": 1999},
        ]})),
    )
    .await;
    mock_get(
        &pms,
        "/library/metadata/10",
        container(json!({"Metadata": [{
            "ratingKey": "10", "type": "movie", "title": "Some Movie", "year": 1999,
            "librarySectionID": 1, "librarySectionTitle": "Movies",
            "Media": [{"container": "mkv", "Part": [
                {"id": 7, "key": "/library/parts/7/1/file.mkv", "size": FILE.len(), "container": "mkv"}
            ]}],
        }]})),
    )
    .await;
    mock_get(
        &pms,
        "/library/parts/7/1/file.mkv",
        ResponseTemplate::new(200).set_body_bytes(FILE),
    )
    .await;

    let plextv = MockServer::start().await;
    mock_get(
        &plextv,
        "/api/v2/resources",
        ResponseTemplate::new(200).set_body_json(json!([{
            "name": "Home Server", "clientIdentifier": SERVER_ID, "provides": "server",
            "owned": true, "productVersion": "1.0", "accessToken": "srv-token",
            "connections": [{"protocol": "http", "address": "127.0.0.1", "port": 1,
                             "uri": pms.uri(), "local": true}],
        }])),
    )
    .await;
    // Keep the PMS alive for the whole test.
    std::mem::forget(pms);

    let dir = tempfile::tempdir().unwrap();
    let store = ConfigStore::in_dir(dir.path().join("config"));
    let mut config = store.load_or_init().unwrap();
    if logged_in {
        config.auth = Some(Auth {
            token: "user-token".into(),
            username: "tester".into(),
        });
        store.save(&config).unwrap();
    }
    let movies = dir.path().join("movies");
    let app = plex_web::router(Settings {
        store: store.clone(),
        layout: Layout {
            movies_dir: movies.clone(),
            tv_dir: dir.path().join("tv"),
        },
        password: password.map(String::from),
        parallel: 2,
        plextv_url: Some(format!("{}/", plextv.uri())),
    });
    Env {
        app,
        plextv,
        store,
        movies,
        _dir: dir,
    }
}

struct Reply {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: String,
}

async fn send(app: &Router, req: Request<Body>) -> Reply {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

async fn get(app: &Router, uri: &str) -> Reply {
    send(app, Request::get(uri).body(Body::empty()).unwrap()).await
}

async fn post(app: &Router, uri: &str) -> Reply {
    let req = Request::post(uri)
        .header(header::HOST, "localhost:8080")
        .header(header::ORIGIN, "http://localhost:8080")
        .body(Body::empty())
        .unwrap();
    send(app, req).await
}

fn location(r: &Reply) -> &str {
    r.headers.get(header::LOCATION).unwrap().to_str().unwrap()
}

#[tokio::test]
async fn signed_out_visitors_are_sent_to_login() {
    let env = env(false, None).await;
    let r = get(&env.app, "/servers").await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(location(&r), "/login");

    let r = get(&env.app, "/login").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("Sign in with Plex"));
}

#[tokio::test]
async fn login_flow_saves_token_and_redirects() {
    let env = env(false, None).await;
    let pin = |token: Value| {
        ResponseTemplate::new(200).set_body_json(json!({
            "id": 5, "code": "abcd", "authToken": token, "expiresIn": 1800,
        }))
    };
    Mock::given(method("POST"))
        .and(path("/api/v2/pins"))
        .respond_with(pin(Value::Null))
        .mount(&env.plextv)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v2/pins/5"))
        .respond_with(pin(Value::Null))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&env.plextv)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v2/pins/5"))
        .respond_with(pin(json!("fresh-token")))
        .with_priority(2)
        .mount(&env.plextv)
        .await;
    mock_get(
        &env.plextv,
        "/api/v2/user",
        ResponseTemplate::new(200).set_body_json(json!({
            "id": 1, "uuid": "u", "username": "newuser", "title": "New",
        })),
    )
    .await;

    let r = post(&env.app, "/login").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("app.plex.tv/auth"));
    assert!(r.body.contains("code=abcd"));

    let r = get(&env.app, "/login/status").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("Waiting"));
    assert!(env.store.load_or_init().unwrap().auth.is_none());

    let r = get(&env.app, "/login/status").await;
    assert_eq!(r.headers.get("hx-redirect").unwrap(), "/servers");
    let auth = env.store.load_or_init().unwrap().auth.unwrap();
    assert_eq!(
        (auth.token.as_str(), auth.username.as_str()),
        ("fresh-token", "newuser")
    );
}

#[tokio::test]
async fn logout_clears_token() {
    let env = env(true, None).await;
    let r = post(&env.app, "/logout").await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(env.store.load_or_init().unwrap().auth.is_none());
}

#[tokio::test]
async fn password_protects_everything() {
    let env = env(true, Some("hunter2")).await;

    let r = get(&env.app, "/servers").await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.headers.contains_key(header::WWW_AUTHENTICATE));

    let basic = |creds: &str| {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(creds);
        Request::get("/servers")
            .header(header::AUTHORIZATION, format!("Basic {encoded}"))
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        send(&env.app, basic("me:wrong")).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&env.app, basic("me:hunter2")).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn cross_site_posts_are_refused() {
    let env = env(true, None).await;

    let evil = Request::post("/logout")
        .header(header::HOST, "localhost:8080")
        .header(header::ORIGIN, "http://evil.example")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&env.app, evil).await.status, StatusCode::FORBIDDEN);

    let fetch_meta = Request::post("/logout")
        .header("sec-fetch-site", "cross-site")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        send(&env.app, fetch_meta).await.status,
        StatusCode::FORBIDDEN
    );

    // Still signed in.
    assert!(env.store.load_or_init().unwrap().auth.is_some());
}

#[tokio::test]
async fn browses_servers_libraries_and_items() {
    let env = env(true, None).await;

    let r = get(&env.app, "/servers").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("Home Server"));
    assert!(r.body.contains("tester"));

    let r = get(&env.app, &format!("/servers/{SERVER_ID}/status")).await;
    assert!(r.body.contains("local"));

    let r = get(&env.app, &format!("/servers/{SERVER_ID}")).await;
    assert!(r.body.contains("Movies"));

    let r = get(&env.app, &format!("/servers/{SERVER_ID}/libraries/1")).await;
    assert!(r.body.contains("Page 1 of 3"));
    // Titles are escaped, not interpreted as HTML.
    assert!(!r.body.contains("<script>alert(1)"));
    assert!(r.body.contains("&lt;script&gt;alert(1)"));
    assert!(
        r.body
            .contains(&format!("/servers/{SERVER_ID}/items/10/cover"))
    );

    let r = get(&env.app, &format!("/servers/{SERVER_ID}/items/10/cover")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(r.body, "fake-jpeg");

    let r = get(&env.app, &format!("/servers/{SERVER_ID}/items/10")).await;
    assert!(r.body.contains("Some Movie (1999)"));
    assert!(r.body.contains("Some Movie (1999).mkv"));
    assert!(r.body.contains("Download"));
}

#[tokio::test]
async fn unknown_server_is_404() {
    let env = env(true, None).await;
    let r = get(&env.app, "/servers/nope").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn download_is_queued_and_saved() {
    let env = env(true, None).await;
    let r = post(&env.app, &format!("/servers/{SERVER_ID}/items/10/download")).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(location(&r), "/downloads");

    let dest = env
        .movies
        .join("Some Movie (1999)")
        .join("Some Movie (1999).mkv");
    for _ in 0..100 {
        if dest.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(std::fs::read(&dest).unwrap(), FILE);

    let r = get(&env.app, "/downloads/list").await;
    assert!(r.body.contains("Done"));
    assert!(r.body.contains("Some Movie (1999).mkv"));

    // Posting again doesn't duplicate or fail: the file is simply already there.
    post(&env.app, &format!("/servers/{SERVER_ID}/items/10/download")).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let r = get(&env.app, "/downloads/list").await;
    assert!(r.body.contains("Already downloaded"));

    let r = post(&env.app, "/downloads/clear").await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let r = get(&env.app, "/downloads/list").await;
    assert!(r.body.contains("No downloads yet"));
}

#[tokio::test]
async fn search_is_scoped_to_server_and_library() {
    let env = env(true, None).await;

    // Search is no longer a top-level nav item; each view has its own box.
    let r = get(&env.app, "/servers").await;
    assert!(!r.body.contains("href=\"/search\""));
    assert!(r.body.contains("action=\"/search\""));
    let r = get(&env.app, &format!("/servers/{SERVER_ID}")).await;
    assert!(
        r.body
            .contains(&format!("name=\"server\" value=\"{SERVER_ID}\""))
    );
    let r = get(&env.app, &format!("/servers/{SERVER_ID}/libraries/1")).await;
    assert!(r.body.contains("name=\"library\" value=\"1\""));

    let r = get(
        &env.app,
        &format!("/search?q=movie&server={SERVER_ID}&library=1"),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("Some Movie (1999)"));
    // Episodes say which show they are from.
    assert!(r.body.contains("Some Movie Show"));
    assert!(r.body.contains("Search Movies"));
}

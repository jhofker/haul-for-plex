use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use haul_core::{Auth, ConfigStore, Layout};
use haul_web::Settings;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SERVER_ID: &str = "srv-1";
const FILE: &[u8] = b"pretend this is a video file";

struct Env {
    app: Router,
    /// The fake Plex server, for tests that need more of it.
    pms: MockServer,
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
    let app = haul_web::router(Settings {
        store: store.clone(),
        layout: Layout {
            movies_dir: movies.clone(),
            tv_dir: dir.path().join("tv"),
        },
        password: password.map(String::from),
        parallel: 2,
        plextv_url: Some(format!("{}/", plextv.uri())),
        retry: Settings::DEFAULT_RETRY,
    });
    Env {
        app,
        pms,
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

#[tokio::test]
async fn download_rejects_unknown_version() {
    let env = env(true, None).await;
    let req = Request::post(format!("/servers/{SERVER_ID}/items/10/download"))
        .header(header::HOST, "localhost:8080")
        .header(header::ORIGIN, "http://localhost:8080")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("version=best"))
        .unwrap();
    let r = send(&env.app, req).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn library_filter_and_sort_go_to_the_server_and_survive_paging() {
    let env = env(true, None).await;
    let pms = &env.pms;
    Mock::given(method("GET"))
        .and(path("/library/sections/1/all"))
        .and(wiremock::matchers::query_param("sort", "year:desc"))
        .and(wiremock::matchers::query_param("title", "alien"))
        .and(wiremock::matchers::query_param("year", "1979"))
        .respond_with(container(json!({"totalSize": 150, "Metadata": [
            {"ratingKey": "5", "type": "movie", "title": "Alien", "year": 1979},
        ]})))
        .with_priority(1)
        .mount(pms)
        .await;

    let r = get(
        &env.app,
        &format!("/servers/{SERVER_ID}/libraries/1?title=alien&year=1979&sort=year:desc"),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("Alien (1979)"));
    assert!(r.body.contains("150 matching items"));
    // The controls show what is applied, and the pager keeps it.
    assert!(r.body.contains("value=\"alien\""));
    assert!(r.body.contains("value=\"1979\""));
    assert!(r.body.contains("<option value=\"year:desc\" selected>"));
    assert!(
        r.body
            .contains("?title=alien&amp;year=1979&amp;sort=year%3Adesc&amp;page=2")
    );

    // Blank form fields are the same as no filter.
    let r = get(
        &env.app,
        &format!("/servers/{SERVER_ID}/libraries/1?title=&year=&sort="),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("250 items"));
    assert!(!r.body.contains(">Clear<"));
}

#[tokio::test]
async fn episodes_can_be_filtered_and_sorted() {
    let env = env(true, None).await;
    let pms = &env.pms;
    mock_get(
        pms,
        "/library/metadata/30",
        container(json!({"Metadata": [{
            "ratingKey": "30", "type": "season", "title": "Season 1", "index": 1,
            "parentTitle": "Some Show", "parentRatingKey": "20",
        }]})),
    )
    .await;
    mock_get(
        pms,
        "/library/metadata/30/children",
        container(json!({"Metadata": [
            {"ratingKey": "31", "type": "episode", "title": "Pilot", "index": 1, "parentIndex": 1,
             "originallyAvailableAt": "2020-01-01"},
            {"ratingKey": "32", "type": "episode", "title": "Second", "index": 2, "parentIndex": 1,
             "originallyAvailableAt": "2020-01-08"},
            {"ratingKey": "33", "type": "episode", "title": "Finale", "index": 3, "parentIndex": 1,
             "originallyAvailableAt": "2020-01-15"},
        ]})),
    )
    .await;

    let r = get(&env.app, &format!("/servers/{SERVER_ID}/items/30")).await;
    assert!(r.body.contains("Episode order"));
    let order = |body: &str| {
        ["Pilot", "Second", "Finale"]
            .map(|t| body.find(&format!("· {t}<")).map(|i| (i, t)))
            .into_iter()
            .flatten()
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_values()
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&r.body), ["Pilot", "Second", "Finale"]);

    let r = get(
        &env.app,
        &format!("/servers/{SERVER_ID}/items/30?sort=released:desc"),
    )
    .await;
    assert_eq!(order(&r.body), ["Finale", "Second", "Pilot"]);

    let r = get(
        &env.app,
        &format!("/servers/{SERVER_ID}/items/30?title=fin"),
    )
    .await;
    assert_eq!(order(&r.body), ["Finale"]);

    let r = get(
        &env.app,
        &format!("/servers/{SERVER_ID}/items/30?title=nope"),
    )
    .await;
    assert!(r.body.contains("Nothing matches these filters."));
}

#[tokio::test]
async fn search_results_can_be_narrowed_by_type_and_sorted() {
    let env = env(true, None).await;
    let base = format!("/search?q=movie&server={SERVER_ID}&library=1");

    let r = get(&env.app, &format!("{base}&type=episode")).await;
    assert!(r.body.contains("Pilot"));
    assert!(!r.body.contains("Some Movie (1999)"));
    assert!(r.body.contains("<option value=\"episode\" selected>"));

    let r = get(&env.app, &format!("{base}&type=show")).await;
    assert!(r.body.contains("No shows found for “movie”."));

    let r = get(&env.app, &format!("{base}&sort=title")).await;
    let movie = r.body.find("Some Movie (1999)").unwrap();
    let pilot = r.body.find("Pilot").unwrap();
    assert!(pilot < movie, "by title, Pilot comes before Some Movie");
}

#[tokio::test]
async fn queue_is_saved_and_jobs_can_be_paused_and_resumed() {
    let env = env(true, None).await;
    // The file hangs, so the job stays in progress.
    Mock::given(method("GET"))
        .and(path("/library/parts/7/1/file.mkv"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(FILE)
                .set_delay(Duration::from_secs(30)),
        )
        .with_priority(1)
        .mount(&env.pms)
        .await;
    post(&env.app, &format!("/servers/{SERVER_ID}/items/10/download")).await;
    let r = get(&env.app, "/downloads/list").await;
    assert!(r.body.contains("/downloads/1/pause"), "{}", r.body);
    assert!(r.body.contains("Pause all"));

    let r = post(&env.app, "/downloads/1/pause").await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let r = get(&env.app, "/downloads/list").await;
    assert!(r.body.contains("Paused"));
    assert!(r.body.contains("/downloads/1/resume"));
    assert!(r.body.contains("Resume all"));

    let queue = env.store.path().with_file_name("queue.json");
    let saved: Value = serde_json::from_slice(&std::fs::read(&queue).unwrap()).unwrap();
    assert_eq!(saved["jobs"][0]["state"], "paused");
    assert_eq!(saved["jobs"][0]["server_id"], SERVER_ID);
    assert_eq!(saved["jobs"][0]["item"], "10");

    let req = Request::post("/downloads/resume")
        .header(header::HOST, "localhost:8080")
        .header(header::ORIGIN, "http://localhost:8080")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("server={SERVER_ID}")))
        .unwrap();
    assert_eq!(send(&env.app, req).await.status, StatusCode::SEE_OTHER);
    let saved: Value = serde_json::from_slice(&std::fs::read(&queue).unwrap()).unwrap();
    assert_eq!(saved["jobs"][0]["state"], "queued");
}

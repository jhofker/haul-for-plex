//! Comparing a shared server with your own library in the web interface.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use haul_core::{Auth, ConfigStore, Layout};
use haul_web::Settings;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MINE: &str = "srv-mine";
const THEIRS: &str = "srv-theirs";

fn container(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "MediaContainer": body }))
}

async fn mock_get(server: &MockServer, p: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(container(body))
        .mount(server)
        .await;
}

fn media(res: &str, part: u64) -> Value {
    json!([{"videoResolution": res, "container": "mkv", "Part": [
        {"id": part, "key": format!("/library/parts/{part}/1/file.mkv"), "size": 4, "container": "mkv"}
    ]}])
}

/// A fake Plex server answering `/identity` as `id`, with one movie and one
/// TV library.
async fn pms(id: &str) -> MockServer {
    let pms = MockServer::start().await;
    mock_get(&pms, "/identity", json!({"machineIdentifier": id})).await;
    mock_get(
        &pms,
        "/library/sections",
        json!({"Directory": [
            {"key": "1", "title": "Movies", "type": "movie"},
            {"key": "2", "title": "TV Shows", "type": "show"},
        ]}),
    )
    .await;
    pms
}

/// Your server: two films (one in 1080p) and two of three episodes of a show.
async fn my_server() -> MockServer {
    let pms = pms(MINE).await;
    mock_get(
        &pms,
        "/library/sections/1/all",
        json!({"totalSize": 2, "Metadata": [
            {"ratingKey": "1", "type": "movie", "title": "Some Movie", "year": 1999,
             "Guid": [{"id": "imdb://tt1"}], "Media": media("1080", 1)},
            {"ratingKey": "2", "type": "movie", "title": "Old Favourite", "year": 1980,
             "Media": media("720", 2)},
        ]}),
    )
    .await;
    // Episodes when asked for type 4; shows otherwise.
    Mock::given(method("GET"))
        .and(path("/library/sections/2/all"))
        .and(query_param("type", "4"))
        .respond_with(container(json!({"totalSize": 2, "Metadata": [
            {"ratingKey": "21", "type": "episode", "title": "Pilot", "index": 1,
             "parentIndex": 1, "grandparentRatingKey": "20", "Media": media("1080", 3)},
            {"ratingKey": "22", "type": "episode", "title": "Second", "index": 2,
             "parentIndex": 1, "grandparentRatingKey": "20", "Media": media("1080", 4)},
        ]})))
        .with_priority(1)
        .mount(&pms)
        .await;
    mock_get(
        &pms,
        "/library/sections/2/all",
        json!({"totalSize": 1, "Metadata": [
            {"ratingKey": "20", "type": "show", "title": "A Show", "year": 2010,
             "Guid": [{"id": "tvdb://77"}]},
        ]}),
    )
    .await;
    pms
}

/// A friend's server: the same film in 4K under another title, a film you
/// don't have, one you do, and the show with a third episode.
async fn their_server() -> MockServer {
    let pms = pms(THEIRS).await;
    let movies = json!([
        {"ratingKey": "10", "type": "movie", "title": "Some Movie (Director's Cut)", "year": 1999,
         "Guid": [{"id": "imdb://tt1"}], "Media": media("4k", 10)},
        {"ratingKey": "11", "type": "movie", "title": "Brand New", "year": 2024, "Media": media("1080", 11)},
        {"ratingKey": "12", "type": "movie", "title": "Old Favourite", "year": 1980, "Media": media("720", 12)},
    ]);
    mock_get(
        &pms,
        "/library/sections/1/all",
        json!({"totalSize": 3, "Metadata": movies}),
    )
    .await;
    let show = json!({"ratingKey": "30", "type": "show", "title": "A Show", "year": 2010,
                      "leafCount": 3, "childCount": 1, "librarySectionID": 2,
                      "librarySectionTitle": "TV Shows",
                      "guid": "com.plexapp.agents.thetvdb://77?lang=en"});
    mock_get(&pms, "/library/metadata/30", json!({"Metadata": [show]})).await;
    let episodes: Vec<Value> = (1..=3)
        .map(|e| {
            json!({"ratingKey": format!("3{e}"), "type": "episode", "title": format!("Episode {e}"),
                   "index": e, "parentIndex": 1, "grandparentTitle": "A Show",
                   "grandparentRatingKey": "30", "parentRatingKey": "39",
                   "Media": media("1080", 30 + e)})
        })
        .collect();
    mock_get(
        &pms,
        "/library/metadata/30/allLeaves",
        json!({"Metadata": episodes}),
    )
    .await;
    mock_get(
        &pms,
        "/library/metadata/30/children",
        json!({"Metadata": [
            {"ratingKey": "39", "type": "season", "title": "Season 1", "index": 1,
             "leafCount": 3, "parentTitle": "A Show", "parentRatingKey": "30"},
        ]}),
    )
    .await;
    for e in 1..=3u64 {
        Mock::given(method("GET"))
            .and(path(format!("/library/parts/{}/1/file.mkv", 30 + e)))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"data".to_vec()))
            .mount(&pms)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/hubs/search"))
        .respond_with(container(
            json!({"Hub": [{"type": "movie", "Metadata": movies}]}),
        ))
        .mount(&pms)
        .await;
    pms
}

fn resource(name: &str, id: &str, owned: bool, pms: &MockServer) -> Value {
    json!({
        "name": name, "clientIdentifier": id, "provides": "server", "owned": owned,
        "sourceTitle": if owned { Value::Null } else { json!("Friend") },
        "productVersion": "1.0", "accessToken": format!("{id}-token"),
        "connections": [{"protocol": "http", "address": "127.0.0.1", "port": 1,
                         "uri": pms.uri(), "local": true}],
    })
}

struct Env {
    app: Router,
    tv: std::path::PathBuf,
    _servers: (MockServer, MockServer, MockServer),
    _dir: tempfile::TempDir,
}

async fn env() -> Env {
    let mine = my_server().await;
    let theirs = their_server().await;
    let plextv = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/resources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            resource("Home", MINE, true, &mine),
            resource("Friend's", THEIRS, false, &theirs),
        ])))
        .mount(&plextv)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let store = ConfigStore::in_dir(dir.path().join("config"));
    let mut config = store.load_or_init().unwrap();
    config.auth = Some(Auth {
        token: "user-token".into(),
        username: "tester".into(),
    });
    store.save(&config).unwrap();
    let tv = dir.path().join("tv");
    let app = haul_web::router(Settings {
        store,
        layout: Layout {
            movies_dir: dir.path().join("movies"),
            tv_dir: tv.clone(),
        },
        password: None,
        parallel: 2,
        plextv_url: Some(format!("{}/", plextv.uri())),
        retry: Settings::DEFAULT_RETRY,
    });
    Env {
        app,
        tv,
        _servers: (mine, theirs, plextv),
        _dir: dir,
    }
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get(app: &Router, uri: &str) -> String {
    let (status, body) = send(app, Request::get(uri).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body
}

async fn post_form(app: &Router, uri: &str, form: &str) -> StatusCode {
    let req = Request::post(uri)
        .header(header::HOST, "localhost:8080")
        .header(header::ORIGIN, "http://localhost:8080")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form.to_string()))
        .unwrap();
    send(app, req).await.0
}

/// Load `uri` until your library has been read in the background.
async fn get_compared(app: &Router, uri: &str) -> String {
    for _ in 0..100 {
        let body = get(app, uri).await;
        if !body.contains("Comparing with your library") {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("your library was never read");
}

/// The table row holding `title`.
fn row<'a>(body: &'a str, title: &str) -> &'a str {
    let at = body
        .find(title)
        .unwrap_or_else(|| panic!("{title} not listed"));
    let end = body[at..].find("</tr>").map_or(body.len(), |e| at + e);
    &body[at..end]
}

#[tokio::test]
async fn marks_what_you_have_and_filters_by_it() {
    let env = env().await;
    let body = get_compared(&env.app, &format!("/servers/{THEIRS}/libraries/1")).await;
    assert!(row(&body, "Some Movie (Director").contains("st-upgrade"));
    assert!(row(&body, "Some Movie (Director").contains("Upgrade: 1080p → 4K"));
    assert!(row(&body, "Brand New").contains("st-missing"));
    assert!(row(&body, "Old Favourite").contains("st-have"));

    let body = get(
        &env.app,
        &format!("/servers/{THEIRS}/libraries/1?status=missing"),
    )
    .await;
    assert!(body.contains("Brand New"));
    assert!(!body.contains("Old Favourite"));
    assert!(body.contains("1 matching items"));

    // Your own server isn't compared with itself.
    let body = get(&env.app, &format!("/servers/{MINE}/libraries/1")).await;
    assert!(!body.contains("st-have"));
}

#[tokio::test]
async fn downloads_just_the_missing_episodes() {
    let env = env().await;
    let body = get_compared(&env.app, &format!("/servers/{THEIRS}/items/30")).await;
    assert!(body.contains("1 episode missing"));
    assert!(body.contains("Download 1 missing episode"));

    let uri = format!("/servers/{THEIRS}/items/30/download");
    assert_eq!(
        post_form(&env.app, &uri, "missing=1").await,
        StatusCode::SEE_OTHER
    );
    let dest = env
        .tv
        .join("A Show (2010)/Season 01/A Show - S01E03 - Episode 3.mkv");
    for _ in 0..100 {
        if dest.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(dest.exists());
    let list = get(&env.app, "/downloads/list").await;
    assert!(!list.contains("S01E01"), "{list}");
}

#[tokio::test]
async fn your_servers_are_hidden_and_can_be_chosen() {
    let env = env().await;
    let body = get(&env.app, "/servers").await;
    assert!(body.contains("Friend&#39;s") || body.contains("Friend's"));
    assert!(body.contains("Your own server is hidden"));
    assert!(!body.contains(&format!("href=\"/servers/{MINE}\"")));
    let body = get(&env.app, "/servers?all=1").await;
    assert!(body.contains(&format!("href=\"/servers/{MINE}\"")));

    // Searching everywhere leaves your server out, with a way back in.
    let body = get_compared(&env.app, "/search?q=brand").await;
    assert!(body.contains("Your own server wasn"));
    assert!(body.contains("mine=1"));

    // Count the friend's server as yours instead.
    assert_eq!(
        post_form(&env.app, "/servers/mine", &format!("server={THEIRS}")).await,
        StatusCode::SEE_OTHER
    );
    let body = get(&env.app, "/servers").await;
    assert!(body.contains(&format!("href=\"/servers/{MINE}\"")));
    assert!(!body.contains(&format!("href=\"/servers/{THEIRS}\"")));
}

use haul_core::{ClientInfo, Error, Resource, ServerClient};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn setup() -> (MockServer, ServerClient) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/identity"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "MediaContainer": {"machineIdentifier": "srv"}
        })))
        .mount(&server)
        .await;
    let resource: Resource = serde_json::from_value(json!({
        "name": "Home", "clientIdentifier": "srv", "provides": "server",
        "accessToken": "server-token",
        "connections": [{"protocol": "http", "address": "127.0.0.1", "port": 1, "uri": server.uri(), "local": true}],
    }))
    .unwrap();
    let client = ServerClient::connect(&resource, &ClientInfo::new("test"), None)
        .await
        .unwrap();
    (server, client)
}

fn container(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "MediaContainer": body }))
}

async fn mount(server: &MockServer, p: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path(p))
        .and(header("X-Plex-Token", "server-token"))
        .respond_with(container(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn lists_libraries() {
    let (server, client) = setup().await;
    mount(
        &server,
        "/library/sections",
        json!({"size": 2, "Directory": [
            {"key": "1", "title": "Movies", "type": "movie", "agent": "tv.plex.agents.movie"},
            {"key": "2", "title": "TV Shows", "type": "show"},
        ]}),
    )
    .await;

    let libs = client.libraries().await.unwrap();
    assert_eq!(libs.len(), 2);
    assert_eq!(libs[1].title, "TV Shows");
    assert_eq!(libs[1].kind, "show");
}

#[tokio::test]
async fn library_items_pages_until_total() {
    let (server, client) = setup().await;
    let item = |i: usize| json!({"ratingKey": i.to_string(), "type": "movie", "title": format!("Movie {i}"), "year": 2000});
    Mock::given(method("GET"))
        .and(path("/library/sections/1/all"))
        .and(header("X-Plex-Container-Start", "0"))
        .respond_with(container(
            json!({"totalSize": 502, "Metadata": (0..500).map(item).collect::<Vec<_>>()}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/library/sections/1/all"))
        .and(header("X-Plex-Container-Start", "500"))
        .respond_with(container(
            json!({"totalSize": 502, "Metadata": [item(500), item(501)]}),
        ))
        .mount(&server)
        .await;

    let items = client.library_items("1").await.unwrap();
    assert_eq!(items.len(), 502);
    assert_eq!(items[501].title, "Movie 501");
}

#[tokio::test]
async fn empty_library_has_no_metadata_key() {
    let (server, client) = setup().await;
    mount(
        &server,
        "/library/sections/3/all",
        json!({"size": 0, "totalSize": 0}),
    )
    .await;
    assert!(client.library_items("3").await.unwrap().is_empty());
}

#[tokio::test]
async fn metadata_includes_media_parts() {
    let (server, client) = setup().await;
    mount(&server, "/library/metadata/100", json!({"Metadata": [{
        "ratingKey": "100", "type": "episode", "title": "Pilot",
        "grandparentTitle": "Some Show", "parentIndex": 1, "index": 1,
        "Media": [{"container": "mkv", "videoResolution": "1080", "Part": [
            {"id": 7, "key": "/library/parts/7/1600000000/file.mkv", "file": "/tv/Some Show/S01E01.mkv", "size": 1000, "container": "mkv"},
            {"id": 8, "key": "/library/parts/8/1600000000/file.mkv", "size": 500},
        ]}],
    }]}))
    .await;

    let ep = client.metadata("100").await.unwrap();
    assert!(ep.is_playable());
    assert_eq!(ep.grandparent_title.as_deref(), Some("Some Show"));
    assert_eq!(ep.media[0].parts.len(), 2);
    assert_eq!(ep.size(), Some(1500));
}

#[tokio::test]
async fn missing_item_is_item_not_found() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/library/metadata/999"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    assert!(matches!(
        client.metadata("999").await,
        Err(Error::ItemNotFound(k)) if k == "999"
    ));
}

#[tokio::test]
async fn children_of_a_show_are_seasons() {
    let (server, client) = setup().await;
    mount(&server, "/library/metadata/50/children", json!({"Metadata": [
        {"ratingKey": "51", "type": "season", "title": "Season 1", "index": 1, "parentTitle": "Some Show", "leafCount": 10},
        {"ratingKey": "52", "type": "season", "title": "Specials", "index": 0, "leafCount": 2},
    ]}))
    .await;

    let seasons = client.children("50").await.unwrap();
    assert_eq!(seasons.len(), 2);
    assert_eq!(seasons[0].leaf_count, Some(10));
    assert!(!seasons[0].is_playable());
}

#[tokio::test]
async fn search_flattens_hubs_and_keeps_video() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/hubs/search"))
        .and(wiremock::matchers::query_param("query", "blade runner"))
        .and(wiremock::matchers::query_param("limit", "10"))
        .and(header("X-Plex-Token", "server-token"))
        .respond_with(container(json!({"size": 4, "Hub": [
            {"type": "movie", "hubIdentifier": "movie", "size": 2, "Metadata": [
                {"ratingKey": "10", "type": "movie", "title": "Blade Runner", "year": 1982, "librarySectionTitle": "Movies"},
                {"ratingKey": "11", "type": "movie", "title": "Blade Runner 2049", "year": 2017},
            ]},
            {"type": "actor", "hubIdentifier": "actor", "size": 1, "Directory": [{"tag": "Harrison Ford"}]},
            {"type": "track", "hubIdentifier": "track", "size": 1, "Metadata": [
                {"ratingKey": "90", "type": "track", "title": "Blade Runner Blues"},
            ]},
            {"type": "show", "hubIdentifier": "show", "size": 0},
        ]})))
        .mount(&server)
        .await;

    let results = client.search("blade runner", 10).await.unwrap();
    let titles: Vec<_> = results.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(titles, ["Blade Runner", "Blade Runner 2049"]);
    assert_eq!(results[0].library_section_title.as_deref(), Some("Movies"));
}

#[tokio::test]
async fn library_page_returns_total() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/library/sections/1/all"))
        .and(header("X-Plex-Container-Start", "100"))
        .and(header("X-Plex-Container-Size", "50"))
        .respond_with(container(json!({
            "totalSize": 230,
            "Metadata": [{"ratingKey": "9", "type": "movie", "title": "Nine", "librarySectionID": 1}],
        })))
        .mount(&server)
        .await;

    let (items, total) = client.library_page("1", 100, 50).await.unwrap();
    assert_eq!(total, 230);
    assert_eq!(items[0].library_section_id, Some(1));
}

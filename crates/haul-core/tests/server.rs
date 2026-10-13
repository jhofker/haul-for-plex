use haul_core::{ClientInfo, Error, Resource, ServerClient, find_server};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MACHINE_ID: &str = "srv-owned";

async fn plex_server(machine_id: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/identity"))
        .and(header("X-Plex-Token", "server-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "MediaContainer": {"size": 0, "machineIdentifier": machine_id, "version": "1.41.0"}
        })))
        .mount(&server)
        .await;
    server
}

fn conn(uri: &str, local: bool, relay: bool) -> serde_json::Value {
    json!({"protocol": "http", "address": "127.0.0.1", "port": 32400, "uri": uri, "local": local, "relay": relay})
}

fn resource(connections: Vec<serde_json::Value>) -> Resource {
    serde_json::from_value(json!({
        "name": "Home Server",
        "clientIdentifier": MACHINE_ID,
        "provides": "server",
        "owned": true,
        "accessToken": "server-token",
        "connections": connections,
    }))
    .unwrap()
}

fn info() -> ClientInfo {
    ClientInfo::new("test-client")
}

#[tokio::test]
async fn prefers_local_over_remote_and_relay() {
    let relay = plex_server(MACHINE_ID).await;
    let remote = plex_server(MACHINE_ID).await;
    let local = plex_server(MACHINE_ID).await;
    let res = resource(vec![
        conn(&relay.uri(), false, true),
        conn(&remote.uri(), false, false),
        conn(&local.uri(), true, false),
    ]);

    let client = ServerClient::connect(&res, &info(), None).await.unwrap();
    assert_eq!(client.reachable().connection.uri, local.uri());
}

#[tokio::test]
async fn skips_dead_and_impostor_connections() {
    let impostor = plex_server("some-other-server").await;
    let remote = plex_server(MACHINE_ID).await;
    let res = resource(vec![
        conn("http://127.0.0.1:9", true, false),
        conn(&impostor.uri(), true, false),
        conn(&remote.uri(), false, false),
    ]);

    let client = ServerClient::connect(&res, &info(), None).await.unwrap();
    assert_eq!(client.reachable().connection.uri, remote.uri());
}

#[tokio::test]
async fn uses_preferred_connection_when_it_works() {
    let remote = plex_server(MACHINE_ID).await;
    let local = plex_server(MACHINE_ID).await;
    let res = resource(vec![
        conn(&local.uri(), true, false),
        conn(&remote.uri(), false, false),
    ]);

    let client = ServerClient::connect(&res, &info(), Some(&remote.uri()))
        .await
        .unwrap();
    assert_eq!(client.reachable().connection.uri, remote.uri());
    assert!(local.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn unreachable_lists_every_attempt() {
    let impostor = plex_server("some-other-server").await;
    let res = resource(vec![
        conn("http://127.0.0.1:9", true, false),
        conn(&impostor.uri(), false, false),
    ]);

    let err = ServerClient::connect(&res, &info(), None)
        .await
        .unwrap_err();
    let Error::Unreachable { attempts, .. } = &err else {
        panic!("unexpected error: {err}");
    };
    assert_eq!(attempts.len(), 2);
    assert!(attempts[1].1.contains("different server"));
    assert!(err.to_string().contains("127.0.0.1:9"));
}

#[test]
fn find_server_matches_id_name_and_prefix() {
    let servers: Vec<Resource> = serde_json::from_value(json!([
        {"name": "Home Server", "clientIdentifier": "a1", "provides": "server"},
        {"name": "Home Theater", "clientIdentifier": "b2", "provides": "server"},
        {"name": "Friend", "clientIdentifier": "c3", "provides": "server"},
    ]))
    .unwrap();

    assert_eq!(find_server(&servers, "b2").unwrap().name, "Home Theater");
    assert_eq!(
        find_server(&servers, "home server").unwrap().name,
        "Home Server"
    );
    assert_eq!(find_server(&servers, "fri").unwrap().name, "Friend");
    assert!(matches!(
        find_server(&servers, "home"),
        Err(Error::AmbiguousServer { .. })
    ));
    assert!(matches!(
        find_server(&servers, "nope"),
        Err(Error::ServerNotFound(_))
    ));
}

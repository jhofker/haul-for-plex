use std::time::Duration;

use haul_core::{ClientInfo, Error, PlexTvClient};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn json(name: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(fixture(name), "application/json")
}

async fn client(server: &MockServer) -> PlexTvClient {
    PlexTvClient::with_base_url(
        ClientInfo::new("test-client"),
        &format!("{}/", server.uri()),
    )
    .unwrap()
}

#[tokio::test]
async fn create_pin_sends_plex_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/pins"))
        .and(query_param("strong", "true"))
        .and(header("X-Plex-Client-Identifier", "test-client"))
        .and(header("X-Plex-Product", "Haul for Plex"))
        .and(header("Accept", "application/json"))
        .respond_with(json("pin.json"))
        .expect(1)
        .mount(&server)
        .await;

    let plex = client(&server).await;
    let pin = plex.create_pin().await.unwrap();
    assert_eq!(pin.id, 12345);
    assert_eq!(pin.code, "abcd1234");
    assert!(pin.auth_token.is_none());

    let url = plex.auth_url(&pin);
    assert!(url.starts_with("https://app.plex.tv/auth#?clientID=test-client&code=abcd1234"));
    assert!(url.contains("context%5Bdevice%5D%5Bproduct%5D=Haul+for+Plex"));
}

#[tokio::test]
async fn user_requires_token() {
    let server = MockServer::start().await;
    let plex = client(&server).await;
    assert!(matches!(plex.user().await, Err(Error::NotLoggedIn)));

    Mock::given(method("GET"))
        .and(path("/api/v2/user"))
        .and(header("X-Plex-Token", "user-token"))
        .respond_with(json("user.json"))
        .mount(&server)
        .await;
    let user = plex.with_token("user-token").user().await.unwrap();
    assert_eq!(user.username, "grabber");
}

#[tokio::test]
async fn servers_filters_non_servers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/resources"))
        .and(header("X-Plex-Token", "tok"))
        .respond_with(json("resources.json"))
        .mount(&server)
        .await;

    let servers = client(&server)
        .await
        .with_token("tok")
        .servers()
        .await
        .unwrap();
    let names: Vec<_> = servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Home Server", "Friend's Server"]);

    let home = &servers[0];
    assert!(home.owned);
    assert_eq!(home.access_token.as_deref(), Some("owned-token"));
    assert_eq!(home.connections.len(), 3);
    assert!(home.connections[0].local);
    assert!(home.connections[2].relay);
    assert_eq!(servers[1].access_token.as_deref(), Some("shared-token"));
}

#[tokio::test]
async fn http_errors_include_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/user"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .with_token("bad")
        .user()
        .await
        .unwrap_err();
    match err {
        Error::Status { status, body, .. } => {
            assert_eq!(status.as_u16(), 401);
            assert_eq!(body, "unauthorized");
        }
        other => panic!("unexpected error: {other}"),
    }
}

fn pin_body(token: Option<&str>) -> ResponseTemplate {
    let mut pin: serde_json::Value = serde_json::from_str(&fixture("pin.json")).unwrap();
    pin["authToken"] = token.into();
    ResponseTemplate::new(200).set_body_json(pin)
}

#[tokio::test]
async fn wait_for_pin_polls_until_approved() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/pins/12345"))
        .respond_with(pin_body(None))
        .up_to_n_times(2)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v2/pins/12345"))
        .respond_with(pin_body(Some("new-token")))
        .with_priority(2)
        .mount(&server)
        .await;

    let plex = client(&server).await;
    let pin: haul_core::plextv::Pin = serde_json::from_str(&fixture("pin.json")).unwrap();
    assert_eq!(pin.expires_in, Some(1800));
    let token = plex
        .wait_for_pin(&pin, Duration::from_millis(10))
        .await
        .unwrap();
    assert_eq!(token, "new-token");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn wait_for_pin_reports_expiry() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/pins/12345"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let plex = client(&server).await;
    let pin: haul_core::plextv::Pin = serde_json::from_str(&fixture("pin.json")).unwrap();
    let err = plex
        .wait_for_pin(&pin, Duration::from_millis(10))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::PinExpired));
}

#[tokio::test]
async fn wait_for_pin_gives_up_after_lifetime() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v2/pins/12345"))
        .respond_with(pin_body(None))
        .mount(&server)
        .await;

    let plex = client(&server).await;
    let mut pin: haul_core::plextv::Pin = serde_json::from_str(&fixture("pin.json")).unwrap();
    pin.expires_in = Some(0);
    let err = plex
        .wait_for_pin(&pin, Duration::from_millis(10))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::PinExpired));
}

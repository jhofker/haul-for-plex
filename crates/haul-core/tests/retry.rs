use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use haul_core::{
    ClientInfo, Error, Outcome, Resource, RetryPolicy, ServerClient, download_with_retry,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PART_KEY: &str = "/library/parts/7/1/file.mkv";
const BODY: &[u8] = b"pretend this is a video file";

fn policy(retries: u32) -> RetryPolicy {
    RetryPolicy {
        retries,
        first_delay: Duration::from_millis(5),
        max_delay: Duration::from_millis(20),
    }
}

async fn server() -> (MockServer, ServerClient) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/identity"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"MediaContainer": {"machineIdentifier": "srv"}})),
        )
        .mount(&server)
        .await;
    let resource: Resource = serde_json::from_value(json!({
        "name": "Home", "clientIdentifier": "srv", "provides": "server", "accessToken": "t",
        "connections": [{"protocol": "http", "address": "127.0.0.1", "port": 1, "uri": server.uri(), "local": true}],
    }))
    .unwrap();
    let client = ServerClient::connect(&resource, &ClientInfo::new("test"), None)
        .await
        .unwrap();
    (server, client)
}

/// The part answers `status` for the first `failures` requests, then the file.
async fn flaky(server: &MockServer, status: u16, failures: u64) {
    if failures > 0 {
        Mock::given(method("GET"))
            .and(path(PART_KEY))
            .respond_with(ResponseTemplate::new(status))
            .up_to_n_times(failures)
            .with_priority(1)
            .mount(server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path(PART_KEY))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY))
        .with_priority(2)
        .mount(server)
        .await;
}

fn part() -> haul_core::Part {
    serde_json::from_value(json!({"id": 7, "key": PART_KEY, "size": BODY.len()})).unwrap()
}

#[tokio::test]
async fn server_errors_are_retried_with_a_fresh_connection() {
    let (server, client) = server().await;
    flaky(&server, 503, 2).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.mkv");

    let connects = AtomicU32::new(0);
    let mut retries = Vec::new();
    let outcome = download_with_retry(
        |attempt| {
            connects.fetch_add(1, Ordering::Relaxed);
            assert_eq!(attempt, connects.load(Ordering::Relaxed) - 1);
            let client = client.clone();
            async move { Ok(client) }
        },
        &part(),
        &dest,
        false,
        &policy(5),
        |_| {},
        |e, _, n| retries.push((n, e.is_retryable())),
    )
    .await
    .unwrap();

    assert_eq!(outcome, Outcome::Downloaded { resumed_from: 0 });
    assert_eq!(std::fs::read(&dest).unwrap(), BODY);
    assert_eq!(retries, [(1, true), (2, true)]);
    assert_eq!(connects.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn gives_up_after_the_retry_limit() {
    let (server, client) = server().await;
    flaky(&server, 502, 10).await;
    let dir = tempfile::tempdir().unwrap();

    let mut retries = 0;
    let err = download_with_retry(
        |_| {
            let client = client.clone();
            async move { Ok(client) }
        },
        &part(),
        &dir.path().join("a.mkv"),
        false,
        &policy(2),
        |_| {},
        |_, _, _| retries += 1,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, Error::Status { status, .. } if status == 502));
    assert_eq!(retries, 2);
}

#[tokio::test]
async fn permanent_errors_are_not_retried() {
    for status in [401, 403, 404] {
        let (server, client) = server().await;
        flaky(&server, status, 10).await;
        let dir = tempfile::tempdir().unwrap();

        let mut retries = 0;
        let err = download_with_retry(
            |_| {
                let client = client.clone();
                async move { Ok(client) }
            },
            &part(),
            &dir.path().join("a.mkv"),
            false,
            &policy(5),
            |_| {},
            |_, _, _| retries += 1,
        )
        .await
        .unwrap_err();
        assert!(!err.is_retryable(), "{status}: {err}");
        assert_eq!(retries, 0, "{status}");
    }
}

#[tokio::test]
async fn unreachable_server_is_retried() {
    let (server, client) = server().await;
    flaky(&server, 200, 0).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.mkv");

    // The first connection attempt fails as if the server were down.
    let outcome = download_with_retry(
        |attempt| {
            let client = client.clone();
            async move {
                if attempt == 0 {
                    Err(Error::Unreachable {
                        server: "Home".into(),
                        attempts: vec![],
                    })
                } else {
                    Ok(client)
                }
            }
        },
        &part(),
        &dest,
        false,
        &policy(1),
        |_| {},
        |_, _, _| {},
    )
    .await
    .unwrap();
    assert_eq!(outcome, Outcome::Downloaded { resumed_from: 0 });
}

#[tokio::test]
async fn dropped_connection_is_retryable() {
    // A server that answers the probe, then hangs up mid-request.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 4096];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            if String::from_utf8_lossy(&buf[..n]).starts_with("GET /identity") {
                let body = r#"{"MediaContainer":{"machineIdentifier":"srv"}}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        }
    });
    let uri = format!("http://127.0.0.1:{port}");
    let resource: Resource = serde_json::from_value(json!({
        "name": "Home", "clientIdentifier": "srv", "provides": "server", "accessToken": "t",
        "connections": [{"protocol": "http", "address": "127.0.0.1", "port": port, "uri": uri, "local": true}],
    }))
    .unwrap();
    let client = ServerClient::connect(&resource, &ClientInfo::new("test"), None)
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let err = client
        .download_part(&part(), &dir.path().join("a.mkv"), false, |_| {})
        .await
        .unwrap_err();
    assert!(err.is_retryable(), "{err:?}");
}

use std::path::Path;
use std::sync::{Arc, Mutex};

use plex_core::{ClientInfo, Error, Layout, Outcome, Resource, ServerClient};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const PART_KEY: &str = "/library/parts/7/1600000000/file.mkv";

/// Deterministic, non-repeating-looking bytes so a wrong offset is detectable.
fn content(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

/// Serves `body`, honoring `Range: bytes=N-` unless `ignore_range` is set.
struct RangeResponder {
    body: Vec<u8>,
    ignore_range: bool,
    seen_ranges: Arc<Mutex<Vec<Option<String>>>>,
}

impl Respond for RangeResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let range = req
            .headers
            .get("range")
            .map(|v| v.to_str().unwrap().to_string());
        self.seen_ranges.lock().unwrap().push(range.clone());

        let start = range
            .as_deref()
            .and_then(|r| r.strip_prefix("bytes="))
            .and_then(|r| r.strip_suffix('-'))
            .and_then(|n| n.parse::<usize>().ok());
        match start {
            Some(start) if !self.ignore_range => {
                if start >= self.body.len() {
                    return ResponseTemplate::new(416);
                }
                ResponseTemplate::new(206)
                    .insert_header(
                        "Content-Range",
                        format!("bytes {start}-{}/{}", self.body.len() - 1, self.body.len()),
                    )
                    .set_body_bytes(self.body[start..].to_vec())
            }
            _ => ResponseTemplate::new(200).set_body_bytes(self.body.clone()),
        }
    }
}

struct Fixture {
    server: MockServer,
    client: ServerClient,
    ranges: Arc<Mutex<Vec<Option<String>>>>,
}

async fn fixture(body: Vec<u8>, ignore_range: bool) -> Fixture {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/identity"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "MediaContainer": {"machineIdentifier": "srv"}
        })))
        .mount(&server)
        .await;
    let ranges = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("GET"))
        .and(path(PART_KEY))
        .and(wiremock::matchers::query_param("download", "1"))
        .respond_with(RangeResponder {
            body,
            ignore_range,
            seen_ranges: ranges.clone(),
        })
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
    Fixture {
        server,
        client,
        ranges,
    }
}

fn part(size: Option<u64>) -> plex_core::Part {
    serde_json::from_value(json!({"id": 7, "key": PART_KEY, "size": size, "container": "mkv"}))
        .unwrap()
}

fn tmp_part(dest: &Path) -> std::path::PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    s.into()
}

#[tokio::test]
async fn downloads_whole_file_and_reports_progress() {
    let body = content(200_000);
    let f = fixture(body.clone(), false).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("Movie (2000)").join("Movie (2000).mkv");

    let mut last = None;
    let outcome = f
        .client
        .download_part(&part(Some(body.len() as u64)), &dest, false, |p| {
            last = Some(p)
        })
        .await
        .unwrap();

    assert_eq!(outcome, Outcome::Downloaded { resumed_from: 0 });
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert!(!tmp_part(&dest).exists());
    let last = last.unwrap();
    assert_eq!(last.downloaded, body.len() as u64);
    assert_eq!(last.total, Some(body.len() as u64));
    assert_eq!(*f.ranges.lock().unwrap(), vec![None]);
}

#[tokio::test]
async fn resumes_from_partial_file() {
    let body = content(100_000);
    let f = fixture(body.clone(), false).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.mkv");
    std::fs::write(tmp_part(&dest), &body[..40_000]).unwrap();

    let outcome = f
        .client
        .download_part(&part(Some(body.len() as u64)), &dest, false, |_| {})
        .await
        .unwrap();

    assert_eq!(
        outcome,
        Outcome::Downloaded {
            resumed_from: 40_000
        }
    );
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert_eq!(
        *f.ranges.lock().unwrap(),
        vec![Some("bytes=40000-".to_string())]
    );
}

#[tokio::test]
async fn restarts_when_server_ignores_range() {
    let body = content(50_000);
    let f = fixture(body.clone(), true).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.mkv");
    std::fs::write(tmp_part(&dest), &body[..10_000]).unwrap();

    let outcome = f
        .client
        .download_part(&part(Some(body.len() as u64)), &dest, false, |_| {})
        .await
        .unwrap();

    assert_eq!(outcome, Outcome::Downloaded { resumed_from: 0 });
    assert_eq!(std::fs::read(&dest).unwrap(), body);
}

#[tokio::test]
async fn partial_longer_than_file_is_discarded() {
    let body = content(1_000);
    let f = fixture(body.clone(), false).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.mkv");
    std::fs::write(tmp_part(&dest), content(5_000)).unwrap();

    f.client
        .download_part(&part(Some(1_000)), &dest, false, |_| {})
        .await
        .unwrap();

    assert_eq!(std::fs::read(&dest).unwrap(), body);
}

#[tokio::test]
async fn finished_partial_is_just_renamed() {
    let body = content(2_000);
    let f = fixture(body.clone(), false).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.mkv");
    std::fs::write(tmp_part(&dest), &body).unwrap();

    let outcome = f
        .client
        .download_part(&part(Some(2_000)), &dest, false, |_| {})
        .await
        .unwrap();

    assert_eq!(
        outcome,
        Outcome::Downloaded {
            resumed_from: 2_000
        }
    );
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert!(f.ranges.lock().unwrap().is_empty());
}

#[tokio::test]
async fn existing_complete_file_is_skipped() {
    let f = fixture(content(100), false).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.mkv");
    std::fs::write(&dest, content(100)).unwrap();

    let outcome = f
        .client
        .download_part(&part(Some(100)), &dest, false, |_| {})
        .await
        .unwrap();

    assert_eq!(outcome, Outcome::Skipped);
    assert!(f.ranges.lock().unwrap().is_empty());
}

#[tokio::test]
async fn existing_file_of_other_size_needs_overwrite() {
    let body = content(100);
    let f = fixture(body.clone(), false).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.mkv");
    std::fs::write(&dest, b"something else").unwrap();

    let err = f
        .client
        .download_part(&part(Some(100)), &dest, false, |_| {})
        .await
        .unwrap_err();
    assert!(matches!(err, Error::DestinationExists(_)));
    assert_eq!(std::fs::read(&dest).unwrap(), b"something else");

    f.client
        .download_part(&part(Some(100)), &dest, true, |_| {})
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body);
}

#[tokio::test]
async fn forbidden_download_is_reported() {
    let f = fixture(content(10), false).await;
    f.server.reset().await;
    Mock::given(method("GET"))
        .and(path(PART_KEY))
        .respond_with(ResponseTemplate::new(403))
        .mount(&f.server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.mkv");

    let err = f
        .client
        .download_part(&part(Some(10)), &dest, false, |_| {})
        .await
        .unwrap_err();
    assert!(matches!(err, Error::DownloadForbidden(name) if name == "Home"));
    assert!(!dest.exists() && !tmp_part(&dest).exists());
}

#[tokio::test]
async fn short_download_keeps_partial_and_reports_mismatch() {
    let f = fixture(content(500), false).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.mkv");

    // The server reports 800 bytes but only has 500.
    let err = f
        .client
        .download_part(&part(Some(800)), &dest, false, |_| {})
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        Error::SizeMismatch {
            expected: 800,
            actual: 500
        }
    ));
    assert!(!dest.exists());
    assert_eq!(std::fs::metadata(tmp_part(&dest)).unwrap().len(), 500);
}

#[tokio::test]
async fn unknown_size_downloads_without_verification() {
    let body = content(3_000);
    let f = fixture(body.clone(), false).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.mkv");

    f.client
        .download_part(&part(None), &dest, false, |_| {})
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body);
}

// ---- planning -------------------------------------------------------------

fn layout() -> Layout {
    Layout {
        movies_dir: "/m".into(),
        tv_dir: "/t".into(),
    }
}

async fn plan_server(routes: Vec<(&str, serde_json::Value)>) -> Fixture {
    let f = fixture(vec![], false).await;
    for (p, body) in routes {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"MediaContainer": body})))
            .mount(&f.server)
            .await;
    }
    f
}

#[tokio::test]
async fn plans_movie_into_movies_dir() {
    let f = plan_server(vec![("/library/metadata/84521", json!({"Metadata": [{
        "ratingKey": "84521", "type": "movie", "title": "The Matrix", "year": 1999,
        "Media": [{"container": "mkv", "Part": [{"id": 1, "key": "/library/parts/1/1/file.mkv", "size": 10, "container": "mkv"}]}],
    }]}))])
    .await;

    let item = f.client.metadata("84521").await.unwrap();
    let plan = f.client.plan_download(&item, &layout()).await.unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(
        plan[0].dest,
        Path::new("/m/The Matrix (1999)/The Matrix (1999).mkv")
    );
}

#[tokio::test]
async fn plans_episode_with_show_year_not_air_year() {
    let f = plan_server(vec![
        ("/library/metadata/100654", json!({"Metadata": [{
            "ratingKey": "100654", "type": "episode", "title": "The Manager and the Salesman",
            "year": 2010, "index": 16, "parentIndex": 6,
            "grandparentTitle": "The Office (US)", "grandparentRatingKey": "100583",
            "Media": [{"container": "mkv", "Part": [{"id": 1, "key": "/library/parts/1/1/file.mkv", "size": 10, "container": "mkv"}]}],
        }]})),
        ("/library/metadata/100583", json!({"Metadata": [{
            "ratingKey": "100583", "type": "show", "title": "The Office (US)", "year": 2005,
        }]})),
    ])
    .await;

    let item = f.client.metadata("100654").await.unwrap();
    let plan = f.client.plan_download(&item, &layout()).await.unwrap();
    assert_eq!(
        plan[0].dest,
        Path::new(
            "/t/The Office (US) (2005)/Season 06/The Office (US) - S06E16 - The Manager and the Salesman.mkv"
        )
    );
}

#[tokio::test]
async fn multi_part_movie_plans_every_part() {
    let f = plan_server(vec![(
        "/library/metadata/5",
        json!({"Metadata": [{
            "ratingKey": "5", "type": "movie", "title": "Epic", "year": 1960,
            "Media": [{"Part": [
                {"id": 1, "key": "/library/parts/1/1/a.avi", "size": 10},
                {"id": 2, "key": "/library/parts/2/1/b.avi", "size": 10},
            ]}],
        }]}),
    )])
    .await;

    let item = f.client.metadata("5").await.unwrap();
    let plan = f.client.plan_download(&item, &layout()).await.unwrap();
    let dests: Vec<_> = plan.iter().map(|p| p.dest.clone()).collect();
    assert_eq!(
        dests,
        [
            Path::new("/m/Epic (1960)/Epic (1960) - pt1.avi"),
            Path::new("/m/Epic (1960)/Epic (1960) - pt2.avi"),
        ]
    );
}

#[tokio::test]
async fn shows_are_not_downloadable_yet() {
    let f = plan_server(vec![(
        "/library/metadata/9",
        json!({"Metadata": [{
            "ratingKey": "9", "type": "show", "title": "Some Show",
        }]}),
    )])
    .await;

    let item = f.client.metadata("9").await.unwrap();
    let err = f.client.plan_download(&item, &layout()).await.unwrap_err();
    assert!(matches!(err, Error::NotDownloadable { .. }));
}

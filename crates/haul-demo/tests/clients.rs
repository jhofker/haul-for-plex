//! The demo servers must keep working with Haul's own clients.

use haul_core::listing::View;
use haul_core::plextv::ClientInfo;
use haul_core::{PlexTvClient, ServerClient, VersionChoice};

async fn connect() -> (haul_demo::Demo, Vec<ServerClient>) {
    let demo = haul_demo::start(1000.0).await.unwrap();
    let tv = PlexTvClient::with_base_url(ClientInfo::new("demo-test"), &demo.plextv_url)
        .unwrap()
        .with_token(haul_demo::TOKEN);
    let resources = tv.servers().await.unwrap();
    assert_eq!(resources.len(), 2);
    let mut clients = Vec::new();
    for r in &resources {
        clients.push(
            ServerClient::connect(r, &ClientInfo::new("demo-test"), None)
                .await
                .unwrap(),
        );
    }
    (demo, clients)
}

#[tokio::test]
async fn browse_search_and_plan() {
    let (demo, clients) = connect().await;
    let atlas = &clients[0];

    let libs = atlas.libraries().await.unwrap();
    assert_eq!(
        libs.iter().map(|l| l.title.as_str()).collect::<Vec<_>>(),
        ["Movies", "TV Shows"]
    );

    let (page, total) = atlas
        .library_page("1", 0, 10, &View::default())
        .await
        .unwrap();
    assert_eq!(page.len(), 10);
    assert!(total > 20);

    // Plex-style search narrows to real title matches.
    let found = atlas.search("orbit seven", 10).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].title, "Orbit Seven");

    // A movie with two versions, and the file names they get.
    let key = demo.servers[0].find("Orbit Seven").unwrap();
    let movie = atlas.metadata(key).await.unwrap();
    assert_eq!(movie.media.len(), 2);
    assert!(!movie.genres.is_empty() && !movie.roles.is_empty());
    let layout = haul_core::Layout {
        movies_dir: "/m".into(),
        tv_dir: "/t".into(),
    };
    let plan = atlas
        .plan_download_version(&movie, &layout, &VersionChoice::Index(2))
        .await
        .unwrap();
    assert!(plan[0].dest.ends_with("Orbit Seven (2021) - 4K.mkv"));

    // A season expands to its episodes.
    let show = demo.servers[0].find("Harbor Lights").unwrap();
    let seasons = atlas.children(show).await.unwrap();
    assert_eq!(seasons.len(), 3);
    let eps = atlas.children(&seasons[0].rating_key).await.unwrap();
    assert_eq!(eps.len(), 8);
    assert_eq!(atlas.all_leaves(show).await.unwrap().len(), 24);
}

#[tokio::test]
async fn covers_ranges_and_forbidden_downloads() {
    let (demo, clients) = connect().await;
    let atlas = &clients[0];
    let key = demo.servers[0].find("Paper Cranes").unwrap();
    let (bytes, kind) = atlas.thumbnail(key, 300, 450).await.unwrap();
    assert_eq!(kind, "image/svg+xml");
    assert!(String::from_utf8(bytes).unwrap().contains("Paper Cranes"));

    // A part can be fetched from an offset, as resuming needs. Only headers
    // are checked: the body would be gigabytes of zeros.
    let movie = atlas.metadata(key).await.unwrap();
    let part = &movie.media[0].parts[0];
    let size = part.size.unwrap();
    let url = atlas
        .base_url()
        .join(part.key.trim_start_matches('/'))
        .unwrap();
    let resp = reqwest::Client::new()
        .get(url)
        .header("Range", "bytes=5000000-")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 206);
    assert_eq!(resp.content_length(), Some(size - 5_000_000));
    assert_eq!(
        resp.headers()["content-range"].to_str().unwrap(),
        format!("bytes 5000000-{}/{size}", size - 1)
    );
    drop(resp);

    // The shared server has a title whose owner turned downloads off.
    let lighthouse = &clients[1];
    let key = demo.servers[1].find("Tidewater").unwrap();
    let movie = lighthouse.metadata(key).await.unwrap();
    let err = lighthouse
        .download_part(
            &movie.media[0].parts[0],
            &std::env::temp_dir().join("haul-demo-never-written.mkv"),
            false,
            |_| {},
        )
        .await
        .unwrap_err();
    assert!(matches!(err, haul_core::Error::DownloadForbidden(_)));
}

#[tokio::test]
async fn compares_the_shared_server_with_yours() {
    use haul_core::{MyLibrary, Status};

    let (demo, clients) = connect().await;
    let tv = PlexTvClient::with_base_url(ClientInfo::new("demo-test"), &demo.plextv_url)
        .unwrap()
        .with_token(haul_demo::TOKEN);
    let resources = tv.servers().await.unwrap();
    let mine = MyLibrary::load(&[(&resources[0], &clients[0])]).await;
    assert!(mine.skipped.is_empty());
    let (movies, shows) = mine.counts();
    assert!(movies > 10 && shows >= 5);

    let lighthouse = &clients[1];
    let status = |title: &str| {
        let key = demo.servers[1].find(title).unwrap().to_string();
        let mine = &mine;
        async move { mine.status(&lighthouse.metadata(&key).await.unwrap()) }
    };
    assert_eq!(status("Paper Cranes").await, Some(Status::Have));
    assert_eq!(status("Copper Moon").await, Some(Status::Missing));
    assert_eq!(
        status("Northbound").await,
        Some(Status::Upgrade {
            mine: 1080,
            theirs: 2160
        })
    );
    assert_eq!(
        status("The Quiet Wing").await,
        Some(Status::Partial { missing: 6 })
    );

    // Downloading what's missing from the show gets just season 4.
    let key = demo.servers[1].find("The Quiet Wing").unwrap();
    let show = lighthouse.metadata(key).await.unwrap();
    let layout = haul_core::Layout {
        movies_dir: "/m".into(),
        tv_dir: "/t".into(),
    };
    let plan = lighthouse
        .plan_download_filtered(
            &show,
            &layout,
            &VersionChoice::First,
            mine.missing_under(&show),
        )
        .await
        .unwrap();
    assert_eq!(plan.len(), 6);
    assert!(
        plan.iter()
            .all(|f| f.dest.to_string_lossy().contains("Season 04"))
    );
}

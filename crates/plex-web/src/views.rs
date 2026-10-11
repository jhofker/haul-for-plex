//! HTML pages, built with maud (which escapes everything interpolated).

use maud::{DOCTYPE, Markup, html};
use plex_core::plextv::{Connection, Resource};
use plex_core::{Library, Metadata, PlannedFile, format_size};

use crate::downloads::{Job, JobState};

pub const PAGE_SIZE: usize = 100;

pub fn layout(title: &str, active: &str, user: Option<&str>, body: Markup) -> Markup {
    let link = |href: &str, label: &str, key: &str| {
        html! { a href=(href) class=[(active == key).then_some("active")] { (label) } }
    };
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " · plex-grabber" }
                link rel="icon" href="data:,";
                link rel="stylesheet" href="/static/app.css";
                script src="/static/theme.js" {}
                script src="/static/htmx.min.js" defer {}
            }
            body {
                header.top {
                    a.brand href="/" { "plex-grabber" }
                    @if user.is_none() { span style="flex:1" {} }
                    @if let Some(user) = user {
                        nav {
                            (link("/servers", "Servers", "servers"))
                            (link("/search", "Search", "search"))
                            (link("/downloads", "Downloads", "downloads"))
                        }
                        form.logout method="post" action="/logout" {
                            span.user { (user) }
                            button.link type="submit" { "Sign out" }
                        }
                    }
                    button.theme type="button" id="theme-toggle" title="Toggle dark mode" aria-label="Toggle dark mode" { "◐" }
                }
                main { (body) }
            }
        }
    }
}

pub fn error_page(user: Option<&str>, heading: &str, message: &str) -> Markup {
    layout(
        heading,
        "",
        user,
        html! {
            h1 { (heading) }
            div.card { p { (message) } p { a href="/servers" { "Back to servers" } } }
        },
    )
}

pub fn login_page(error: Option<&str>) -> Markup {
    layout(
        "Sign in",
        "",
        None,
        html! {
            h1 { "Sign in" }
            div.card {
                p { "Sign in with your Plex account to browse and download from the servers you can access." }
                @if let Some(error) = error { p.bad { (error) } }
                form method="post" action="/login" {
                    button.primary type="submit" { "Sign in with Plex" }
                }
            }
        },
    )
}

/// The waiting panel; polls until the login is approved.
pub fn login_waiting(auth_url: &str, note: Option<&str>) -> Markup {
    html! {
        div #login-wait hx-get="/login/status" hx-trigger="every 2s" hx-swap="outerHTML" {
            @if let Some(note) = note { p.muted { (note) } }
            p { "Waiting for you to approve the sign-in on plex.tv…" }
            p { a.button.primary href=(auth_url) target="_blank" rel="noopener" { "Open Plex to sign in" } }
        }
    }
}

pub fn login_waiting_page(auth_url: &str) -> Markup {
    layout(
        "Sign in",
        "",
        None,
        html! {
            h1 { "Sign in" }
            div.card { (login_waiting(auth_url, None)) }
        },
    )
}

pub fn login_expired() -> Markup {
    html! {
        div #login-wait {
            p.bad { "That sign-in link expired." }
            form method="post" action="/login" { button.primary type="submit" { "Try again" } }
        }
    }
}

fn owner(server: &Resource) -> String {
    if server.owned {
        "you".into()
    } else {
        server
            .source_title
            .clone()
            .unwrap_or_else(|| "shared".into())
    }
}

pub fn connection_kind(conn: &Connection) -> &'static str {
    match (conn.relay, conn.local) {
        (true, _) => "relay",
        (false, true) => "local",
        (false, false) => "remote",
    }
}

pub fn servers_page(user: &str, servers: &[Resource]) -> Markup {
    layout(
        "Servers",
        "servers",
        Some(user),
        html! {
            h1 { "Servers" }
            @if servers.is_empty() {
                p.empty { "No servers are shared with this Plex account." }
            } @else {
                table {
                    thead { tr { th { "Name" } th { "Owner" } th { "Version" } th { "Connection" } } }
                    tbody {
                        @for s in servers {
                            tr {
                                td { a href={"/servers/" (s.client_identifier)} { (s.name) } }
                                td { (owner(s)) }
                                td.muted { (s.product_version) }
                                td hx-get={"/servers/" (s.client_identifier) "/status"}
                                   hx-trigger="load" hx-swap="innerHTML" { span.muted { "checking…" } }
                            }
                        }
                    }
                }
            }
        },
    )
}

pub fn server_status(result: Result<(&Connection, u128), String>) -> Markup {
    match result {
        Ok((conn, ms)) => html! {
            span.ok { (connection_kind(conn)) } " " span.muted { (ms) " ms" }
        },
        Err(message) => html! { span.bad title=(message) { "unreachable" } },
    }
}

pub fn libraries_page(user: &str, server: &Resource, libraries: &[Library]) -> Markup {
    layout(
        &server.name,
        "servers",
        Some(user),
        html! {
            div.crumbs { a href="/servers" { "Servers" } }
            h1 { (server.name) }
            @if libraries.is_empty() {
                p.empty { "This server has no libraries you can access." }
            } @else {
                table {
                    thead { tr { th { "Library" } th { "Type" } } }
                    tbody {
                        @for lib in libraries {
                            tr {
                                td { a href={"/servers/" (server.client_identifier) "/libraries/" (lib.key)} { (lib.title) } }
                                td { span.badge { (lib.kind) } }
                            }
                        }
                    }
                }
            }
        },
    )
}

/// `Title (Year)`, or for episodes `S01E02 · Title`.
fn item_title(m: &Metadata) -> String {
    match (m.kind.as_str(), m.parent_index, m.index) {
        ("episode", Some(season), Some(ep)) => format!("S{season:02}E{ep:02} · {}", m.title),
        _ => match m.year {
            Some(y) if m.kind == "movie" || m.kind == "show" => format!("{} ({y})", m.title),
            _ => m.title.clone(),
        },
    }
}

fn item_detail(m: &Metadata) -> String {
    match m.kind.as_str() {
        "show" => match (m.child_count, m.leaf_count) {
            (Some(s), Some(e)) => format!("{s} seasons · {e} episodes"),
            _ => String::new(),
        },
        "season" => m
            .leaf_count
            .map(|e| format!("{e} episodes"))
            .unwrap_or_default(),
        _ => m.size().map(format_size).unwrap_or_default(),
    }
}

fn items_table(server_id: &str, items: &[Metadata]) -> Markup {
    html! {
        @if items.is_empty() {
            p.empty { "Nothing here." }
        } @else {
            table {
                thead { tr { th { "Title" } th { "Type" } th { "Details" } } }
                tbody {
                    @for m in items {
                        tr {
                            td { a href={"/servers/" (server_id) "/items/" (m.rating_key)} { (item_title(m)) } }
                            td { span.badge { (m.kind) } }
                            td.muted { (item_detail(m)) }
                        }
                    }
                }
            }
        }
    }
}

pub fn library_page(
    user: &str,
    server: &Resource,
    library_key: &str,
    library_title: &str,
    items: &[Metadata],
    page: usize,
    total: usize,
) -> Markup {
    let pages = total.div_ceil(PAGE_SIZE).max(1);
    let base = format!(
        "/servers/{}/libraries/{library_key}",
        server.client_identifier
    );
    layout(
        library_title,
        "servers",
        Some(user),
        html! {
            div.crumbs {
                a href="/servers" { "Servers" } " › "
                a href={"/servers/" (server.client_identifier)} { (server.name) }
            }
            h1 { (library_title) }
            p.muted { (total) " items" }
            (items_table(&server.client_identifier, items))
            @if pages > 1 {
                div.pager {
                    @if page > 1 { a href={(base) "?page=" (page - 1)} { "← Previous" } }
                    span.muted { "Page " (page) " of " (pages) }
                    @if page < pages { a href={(base) "?page=" (page + 1)} { "Next →" } }
                }
            }
        },
    )
}

pub struct ItemPage<'a> {
    pub user: &'a str,
    pub server: &'a Resource,
    pub item: &'a Metadata,
    pub crumbs: Vec<(String, String)>,
    pub children: &'a [Metadata],
    /// What a download would save; empty if the item can't be downloaded.
    pub plan: &'a [PlannedFile],
}

pub fn item_page(p: &ItemPage<'_>) -> Markup {
    let id = &p.server.client_identifier;
    let total: u64 = p.plan.iter().filter_map(|f| f.part.size).sum();
    layout(
        &p.item.title,
        "servers",
        Some(p.user),
        html! {
            div.crumbs {
                a href="/servers" { "Servers" } " › "
                a href={"/servers/" (id)} { (p.server.name) }
                @for (href, label) in &p.crumbs { " › " a href=(href) { (label) } }
            }
            h1 { (item_title(p.item)) }
            p { span.badge { (p.item.kind) } " " span.muted { (item_detail(p.item)) } }

            @if !p.plan.is_empty() {
                div.card {
                    form method="post" action={"/servers/" (id) "/items/" (p.item.rating_key) "/download"} {
                        button.primary type="submit" {
                            @if p.plan.len() == 1 { "Download" }
                            @else { "Download all " (p.plan.len()) " files (" (format_size(total)) ")" }
                        }
                    }
                    p.muted { "Saves to:" }
                    @for f in p.plan.iter().take(5) {
                        div.path { (f.dest.display()) }
                    }
                    @if p.plan.len() > 5 { div.muted { "…and " (p.plan.len() - 5) " more" } }
                }
            }

            @if !p.children.is_empty() { (items_table(id, p.children)) }
        },
    )
}

pub struct SearchResults<'a> {
    pub found: &'a [(Resource, Vec<Metadata>)],
    /// Names of servers that could not be searched.
    pub skipped: &'a [String],
}

pub fn search_page(user: &str, query: &str, results: Option<SearchResults<'_>>) -> Markup {
    layout(
        "Search",
        "search",
        Some(user),
        html! {
            h1 { "Search" }
            form.row method="get" action="/search" {
                input type="search" name="q" value=(query) placeholder="Movie or show title" autofocus;
                button.primary type="submit" { "Search" }
            }
            @if let Some(results) = results {
                @for name in results.skipped { p.muted { "Skipped " (name) " (unreachable)" } }
                @if results.found.iter().all(|(_, items)| items.is_empty()) {
                    p.empty { "No results for “" (query) "”." }
                }
                @for (server, items) in results.found {
                    @if !items.is_empty() {
                        h2 { (server.name) }
                        (items_table(&server.client_identifier, items))
                    }
                }
            }
        },
    )
}

fn job_row(job: &Job) -> Markup {
    let name = job
        .dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    html! {
        tr {
            td { div { (name) } div.path.muted { (job.dest.display()) } }
            td { (job.server) }
            td.status {
                @match &job.state {
                    JobState::Queued => { span.muted { "Queued" } }
                    JobState::Downloading { downloaded, total, base, started } => {
                        @let secs = started.elapsed().as_secs_f64().max(0.001);
                        @let speed = (downloaded.saturating_sub(*base)) as f64 / secs;
                        @match total {
                            Some(t) => { progress value=(downloaded) max=(t) {} }
                            None => { progress {} }
                        }
                        span.muted {
                            (format_size(*downloaded))
                            @if let Some(t) = total { " / " (format_size(*t)) }
                            " · " (format_size(speed as u64)) "/s"
                        }
                    }
                    JobState::Done { bytes, resumed_from } => {
                        span.ok { "Done" } " " span.muted { (format_size(*bytes)) }
                        @if *resumed_from > 0 { span.muted { " (resumed)" } }
                    }
                    JobState::Skipped => { span.muted { "Already downloaded" } }
                    JobState::Failed(message) => { span.bad { (message) } }
                }
            }
            td {
                @if job.state.is_active() {
                    form.inline method="post" action={"/downloads/" (job.id) "/cancel"} {
                        button.link type="submit" { "Cancel" }
                    }
                }
            }
        }
    }
}

/// The table that is polled for updates.
pub fn downloads_list(jobs: &[Job]) -> Markup {
    html! {
        div #downloads hx-get="/downloads/list" hx-trigger="every 2s" hx-swap="outerHTML" {
            @if jobs.is_empty() {
                p.empty { "No downloads yet. Open an item and press Download." }
            } @else {
                table {
                    thead { tr { th { "File" } th { "Server" } th { "Status" } th {} } }
                    tbody { @for job in jobs.iter().rev() { (job_row(job)) } }
                }
                @if jobs.iter().any(|j| !j.state.is_active()) {
                    form method="post" action="/downloads/clear" style="margin-top:1rem" {
                        button type="submit" { "Clear finished" }
                    }
                }
            }
        }
    }
}

pub fn downloads_page(user: &str, jobs: &[Job]) -> Markup {
    layout(
        "Downloads",
        "downloads",
        Some(user),
        html! {
            h1 { "Downloads" }
            (downloads_list(jobs))
        },
    )
}

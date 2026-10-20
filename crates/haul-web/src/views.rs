//! HTML pages, built with maud (which escapes everything interpolated).

use haul_core::library::{Media, Part, Stream, Tag};
use haul_core::mine::height_label;
use haul_core::plextv::{Connection, Resource};
use haul_core::{Library, Metadata, PlannedFile, Status, StatusFilter, format_size};
use maud::{DOCTYPE, Markup, html};

use crate::downloads::{Job, JobState};
use crate::listing::{ListParams, href};
use crate::state::Mine;

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
                title { (title) " · Haul for Plex" }
                link rel="icon" href="data:,";
                link rel="stylesheet" href="/static/app.css";
                script src="/static/theme.js" {}
                script src="/static/htmx.min.js" defer {}
            }
            body {
                header.top {
                    a.brand href="/" { "Haul for Plex" }
                    @if user.is_none() { span style="flex:1" {} }
                    @if let Some(user) = user {
                        nav {
                            (link("/servers", "Servers", "servers"))
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

/// The servers page: other people's servers, and which of yours count as
/// your library.
pub struct ServersPage<'a> {
    pub user: &'a str,
    pub servers: &'a [Resource],
    /// Machine ids of the servers that count as your library.
    pub mine: &'a [String],
    /// List your own servers too.
    pub show_all: bool,
    pub library: &'a Mine,
    /// Your library is being read right now.
    pub refreshing: bool,
}

pub fn servers_page(p: &ServersPage<'_>) -> Markup {
    let is_mine = |s: &Resource| p.mine.contains(&s.client_identifier);
    // Downloading from yourself is rarely wanted, so your servers are
    // hidden unless asked for (or there's nothing else).
    let others = p.servers.iter().filter(|s| !is_mine(s)).count();
    let hide_mine = !p.show_all && others > 0;
    let listed: Vec<&Resource> = p
        .servers
        .iter()
        .filter(|s| !(hide_mine && is_mine(s)))
        .collect();
    let hidden = p.servers.len() - listed.len();
    // Offer your own servers as choices; with none, any server.
    let owned = p.servers.iter().any(|s| s.owned);
    let choices: Vec<&Resource> = p
        .servers
        .iter()
        .filter(|s| !owned || s.owned || is_mine(s))
        .collect();
    layout(
        "Servers",
        "servers",
        Some(p.user),
        html! {
            h1 { "Servers" }
            @if !p.servers.is_empty() {
                (search_form(SearchScope::default(), "", "Search all servers", None, false))
            }
            @if p.servers.is_empty() {
                p.empty { "No servers are shared with this Plex account." }
            } @else {
                table {
                    thead { tr { th { "Name" } th { "Owner" } th { "Version" } th { "Connection" } } }
                    tbody {
                        @for s in &listed {
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
                @if hidden > 0 {
                    p.muted {
                        "Your own " @if hidden == 1 { "server is" } @else { "servers are" } " hidden. "
                        a href="/servers?all=1" { "Show" }
                    }
                } @else if p.show_all && others > 0 && p.servers.len() > others {
                    p.muted { a href="/servers" { "Hide your own servers" } }
                }
                (my_library_card(p, &choices))
            }
        },
    )
}

fn my_library_card(p: &ServersPage<'_>, choices: &[&Resource]) -> Markup {
    html! {
        div.card #my-library {
            h2 { "My library" }
            p.muted {
                "Haul compares other servers with these, to show what you already have, "
                "what's missing and where a better copy is on offer."
            }
            form method="post" action="/servers/mine" {
                @for s in choices {
                    div {
                        label {
                            input type="checkbox" name="server" value=(s.client_identifier)
                                checked[p.mine.contains(&s.client_identifier)];
                            " " (s.name)
                            @if !s.owned { " " span.muted { "(" (owner(s)) ")" } }
                        }
                    }
                }
                div.row style="margin-top:.5rem" {
                    button type="submit" { "Save" }
                }
            }
            @match p.library {
                Mine::Ready(lib) => {
                    @let (movies, shows) = lib.counts();
                    p.muted {
                        (movies) " movies and " (shows) " shows, read "
                        (ago(lib.age().as_secs())) "."
                        @if p.refreshing { " Reading again…" }
                    }
                    @for name in &lib.skipped { p.bad { "Couldn't read " (name) "." } }
                }
                Mine::Loading => { p.muted { "Reading your library…" } }
                Mine::Off => {}
            }
            @if !matches!(p.library, Mine::Off) {
                form method="post" action="/servers/mine/refresh" {
                    button.link type="submit" { "Read again now" }
                }
            }
        }
    }
}

/// `just now`, `5 minutes ago`, `2 hours ago`.
fn ago(secs: u64) -> String {
    match secs {
        0..60 => "just now".into(),
        60..120 => "a minute ago".into(),
        120..3600 => format!("{} minutes ago", secs / 60),
        3600..7200 => "an hour ago".into(),
        _ => format!("{} hours ago", secs / 3600),
    }
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
            (search_form(
                SearchScope { server: Some(&server.client_identifier), library: None, include_mine: false },
                "",
                &format!("Search {}", server.name),
                None,
                false,
            ))
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

/// The show an episode or season belongs to: its rating key (if known) and title.
fn parent_show(m: &Metadata) -> Option<(Option<&str>, &str)> {
    match m.kind.as_str() {
        "episode" => Some((
            m.grandparent_rating_key.as_deref(),
            m.grandparent_title.as_deref()?,
        )),
        "season" => Some((m.parent_rating_key.as_deref(), m.parent_title.as_deref()?)),
        _ => None,
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

/// Cover image; hidden by theme.js if the server has none.
fn cover(server_id: &str, m: &Metadata, large: bool) -> Markup {
    let wide = m.kind == "episode";
    html! {
        img.cover.wide[wide].large[large]
            src={"/servers/" (server_id) "/items/" (m.rating_key) "/cover" @if wide { "?wide=true" }}
            alt="" loading="lazy";
    }
}

/// `context` adds the show an episode or season belongs to (for search results,
/// where items aren't listed under their show).
/// `empty` is shown instead of the table when there are no items.
/// `statuses`, when comparing with your library, holds each item's status.
fn items_table(
    server_id: &str,
    items: &[Metadata],
    statuses: Option<&[Option<Status>]>,
    context: bool,
    empty: &str,
) -> Markup {
    html! {
        @if items.is_empty() {
            p.empty { (empty) }
        } @else {
            table {
                thead { tr {
                    th { "Title" }
                    @if statuses.is_some() { th { "Your library" } }
                    th { "Type" } th { "Details" }
                } }
                tbody {
                    @for (i, m) in items.iter().enumerate() {
                        tr {
                            td {
                                div.titlecell {
                                    @if matches!(m.kind.as_str(), "movie" | "show" | "season") {
                                        (cover(server_id, m, false))
                                    }
                                    div {
                                        a href={"/servers/" (server_id) "/items/" (m.rating_key)} { (item_title(m)) }
                                        @if context {
                                            @if let Some((key, name)) = parent_show(m) {
                                                div.muted {
                                                    "from "
                                                    @if let Some(key) = key {
                                                        a href={"/servers/" (server_id) "/items/" (key)} { (name) }
                                                    } @else { (name) }
                                                    @if m.kind == "episode" {
                                                        @if let Some(season) = m.parent_index { " · Season " (season) }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            @if let Some(statuses) = statuses {
                                td { @if let Some(status) = statuses[i] { (status_badge(status)) } }
                            }
                            td { span.badge { (m.kind) } }
                            td.muted { (item_detail(m)) }
                        }
                    }
                }
            }
        }
    }
}

/// How an item compares with your library, as a coloured badge.
pub fn status_badge(status: Status) -> Markup {
    let class = format!("badge st-{}", status.name());
    let label = match status {
        Status::Upgrade { .. } => "Upgrade".to_string(),
        other => other.label(),
    };
    html! {
        span class=(class) title=[matches!(status, Status::Upgrade { .. }).then(|| status.label())] { (label) }
    }
}

/// A line saying your library is still being read, when it is.
fn mine_note(mine: &Mine) -> Markup {
    html! {
        @if matches!(mine, Mine::Loading) {
            p.muted.note { "Comparing with your library… reload in a moment to see what you already have." }
        }
    }
}

pub struct LibraryPage<'a> {
    pub user: &'a str,
    pub server: &'a Resource,
    pub library_key: &'a str,
    pub library_title: &'a str,
    pub items: &'a [Metadata],
    pub page: usize,
    /// How many items match the filter, across all pages.
    pub total: usize,
    pub params: &'a ListParams,
    pub mine: &'a Mine,
    /// Each item's status, when comparing with your library.
    pub statuses: Option<&'a [Option<Status>]>,
}

pub fn library_page(p: &LibraryPage<'_>) -> Markup {
    let LibraryPage {
        user,
        server,
        library_key,
        library_title,
        items,
        page,
        total,
        params,
        mine,
        statuses,
    } = *p;
    let pages = total.div_ceil(PAGE_SIZE).max(1);
    let base = format!(
        "/servers/{}/libraries/{library_key}",
        server.client_identifier
    );
    let page_link = |n: usize| {
        let mut pairs = params.pairs();
        pairs.push(("page", n.to_string()));
        href(&base, &pairs)
    };
    let filtered =
        params.view().filter != haul_core::Filter::default() || params.status().is_some();
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
            (search_form(
                SearchScope { server: Some(&server.client_identifier), library: Some(library_key), include_mine: false },
                "",
                &format!("Search {library_title}"),
                None,
                false,
            ))
            (list_controls(&ListControls {
                action: &base,
                params,
                sorts: LIBRARY_SORTS,
                default_sort: "Title, A–Z",
                title: true,
                year: true,
                status: !matches!(mine, Mine::Off),
            }))
            (mine_note(mine))
            p.muted { (total) @if filtered { " matching" } " items" }
            (items_table(
                &server.client_identifier,
                items,
                statuses,
                false,
                if filtered { "Nothing matches these filters." } else { "Nothing here." },
            ))
            @if pages > 1 {
                div.pager {
                    @if page > 1 { a href=(page_link(page - 1)) { "← Previous" } }
                    span.muted { "Page " (page) " of " (pages) }
                    @if page < pages { a href=(page_link(page + 1)) { "Next →" } }
                }
            }
        },
    )
}

/// A sort menu entry: the `sort` value and its label. The list's own order
/// (an empty value) comes first and is labelled per list.
type SortOption = (&'static str, &'static str);

const LIBRARY_SORTS: &[SortOption] = &[
    ("title:desc", "Title, Z–A"),
    ("year:desc", "Year, newest first"),
    ("year", "Year, oldest first"),
    ("added:desc", "Recently added"),
    ("released:desc", "Release date, newest first"),
    ("rating:desc", "Highest rated"),
];

const CHILD_SORTS: &[SortOption] = &[
    ("title", "Title, A–Z"),
    ("title:desc", "Title, Z–A"),
    ("released:desc", "Air date, newest first"),
    ("released", "Air date, oldest first"),
    ("added:desc", "Recently added"),
    ("rating:desc", "Highest rated"),
];

const SEARCH_SORTS: &[SortOption] = &[
    ("title", "Title, A–Z"),
    ("year:desc", "Year, newest first"),
    ("year", "Year, oldest first"),
    ("added:desc", "Recently added"),
    ("rating:desc", "Highest rated"),
];

const KINDS: [(&str, &str); 4] = [
    ("movie", "Movies"),
    ("show", "Shows"),
    ("season", "Seasons"),
    ("episode", "Episodes"),
];

fn kind_label(params: &ListParams) -> Option<&'static str> {
    let kind = params.view().filter.kind?;
    KINDS.iter().find(|(v, _)| *v == kind).map(|(_, l)| *l)
}

/// `current` is the normalised sort value; one not in `options` (from a
/// hand-edited URL) still gets an entry so the menu matches the list.
fn sort_select(options: &[SortOption], default_label: &str, current: &str) -> Markup {
    let known = current.is_empty() || options.iter().any(|(v, _)| *v == current);
    html! {
        select name="sort" aria-label="Sort" {
            option value="" selected[current.is_empty()] { (default_label) }
            @for (value, label) in options {
                option value=(value) selected[*value == current] { (label) }
            }
            @if !known { option value=(current) selected { (current) } }
        }
    }
}

const STATUSES: [(StatusFilter, &str); 3] = [
    (StatusFilter::Missing, "Missing from my library"),
    (StatusFilter::Upgrade, "Upgrades"),
    (StatusFilter::Have, "Already have"),
];

fn status_select(params: &ListParams) -> Markup {
    let current = params.status();
    html! {
        select name="status" aria-label="Compared with my library" {
            option value="" selected[current.is_none()] { "In my library or not" }
            @for (value, label) in STATUSES {
                option value=(value.name()) selected[current == Some(value)] { (label) }
            }
        }
    }
}

fn kind_select(params: &ListParams) -> Markup {
    let current = params.view().filter.kind.unwrap_or_default();
    html! {
        select name="type" aria-label="Type" {
            option value="" selected[current.is_empty()] { "All types" }
            @for (value, label) in KINDS {
                option value=(value) selected[value == current] { (label) }
            }
        }
    }
}

struct ListControls<'a> {
    /// The page's own path; the form reloads it with the chosen settings.
    action: &'a str,
    params: &'a ListParams,
    sorts: &'a [SortOption],
    /// Label for the list's own order.
    default_sort: &'a str,
    /// Offer a title filter box.
    title: bool,
    /// Offer a year box.
    year: bool,
    /// Offer a menu comparing with your library.
    status: bool,
}

/// Filter and sort controls above a list. Menus apply as soon as they
/// change; text boxes on Enter or Apply.
fn list_controls(c: &ListControls<'_>) -> Markup {
    let view = c.params.view();
    html! {
        form.row.listctl method="get" action=(c.action)
            hx-get=(c.action) hx-target="main" hx-select="main" hx-swap="outerHTML"
            hx-push-url="true" data-autosubmit {
            @if c.title {
                input type="search" name="title" value=(view.filter.text.as_deref().unwrap_or(""))
                    placeholder="Filter titles" aria-label="Filter titles";
            }
            @if c.year {
                input.year type="number" name="year" min="1800" max="2200"
                    value=(view.filter.year.map(|y| y.to_string()).unwrap_or_default())
                    placeholder="Year" aria-label="Year";
            }
            (sort_select(c.sorts, c.default_sort, &c.params.sort_value()))
            @if c.status { (status_select(c.params)) }
            button type="submit" { "Apply" }
            @if !c.params.is_default() { a href=(c.action) { "Clear" } }
        }
    }
}

pub struct ItemPage<'a> {
    pub user: &'a str,
    pub server: &'a Resource,
    pub item: &'a Metadata,
    pub crumbs: Vec<(String, String)>,
    /// Seasons or episodes, already filtered and sorted.
    pub children: &'a [Metadata],
    /// How many children there are before filtering.
    pub child_count: usize,
    pub params: &'a ListParams,
    /// What a download would save; empty if the item can't be downloaded.
    pub plan: &'a [PlannedFile],
    /// For a movie or episode with several versions: each version's number
    /// (1-based) and what downloading it would save.
    pub version_plans: &'a [(usize, Vec<PlannedFile>)],
    pub mine: &'a Mine,
    /// How the item compares with your library.
    pub status: Option<Status>,
    /// Each child's status, when comparing.
    pub child_statuses: Option<&'a [Option<Status>]>,
    /// For a show or season you partly have: what downloading just the
    /// missing episodes would save.
    pub missing_plan: &'a [PlannedFile],
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
            div.hero {
                (cover(id, p.item, true))
                div {
                    h1 { (item_title(p.item)) }
                    p {
                        span.badge { (p.item.kind) } " "
                        @if let Some(status) = p.status { (status_badge(status)) " " }
                        span.muted { (item_detail(p.item)) }
                    }
                    @if let Some(Status::Upgrade { mine, theirs }) = p.status {
                        p.muted { "Your library has this in " (height_label(mine)) "; this copy is " (height_label(theirs)) "." }
                    }
                    (about(p.item))
                }
            }

            @if !p.missing_plan.is_empty() {
                div.card {
                    form method="post" action={"/servers/" (id) "/items/" (p.item.rating_key) "/download"} {
                        input type="hidden" name="missing" value="1";
                        button.primary type="submit" {
                            "Download " (p.missing_plan.len()) " missing "
                            @if p.missing_plan.len() == 1 { "episode" } @else { "episodes" }
                            " (" (format_size(p.missing_plan.iter().filter_map(|f| f.part.size).sum())) ")"
                        }
                    }
                    p.muted { "Only the episodes your library doesn't have." }
                }
            }
            (mine_note(p.mine))

            @if !p.plan.is_empty() {
                div.card {
                    form method="post" action={"/servers/" (id) "/items/" (p.item.rating_key) "/download"} {
                        @if p.version_plans.len() > 1 {
                            // Several versions: one button each, saved side by side.
                            @for (n, files) in p.version_plans {
                                @let media = &p.item.media[n - 1];
                                div.version {
                                    button.primary type="submit" name="version" value=(n) {
                                        "Download " (media.summary())
                                    }
                                    " " span.muted { (format_size(files.iter().filter_map(|f| f.part.size).sum())) }
                                    @for f in files { div.path { (f.dest.display()) } }
                                }
                            }
                        } @else {
                            @if matches!(p.item.kind.as_str(), "show" | "season") {
                                div.row {
                                    label.muted for="version" { "Version" }
                                    select #version name="version" {
                                        option value="first" { "First listed" }
                                        option value="4k" { "4K" }
                                        option value="1080" { "1080p" }
                                        option value="720" { "720p" }
                                        option value="sd" { "SD" }
                                    }
                                    span.muted { "Episodes without it use their first version." }
                                }
                            }
                            button.primary type="submit" {
                                @if p.plan.len() == 1 { "Download" }
                                @else { "Download all " (p.plan.len()) " files (" (format_size(total)) ")" }
                            }
                        }
                    }
                    @if p.version_plans.len() <= 1 {
                        p.muted { "Saves to:" }
                        @for f in p.plan.iter().take(5) {
                            div.path { (f.dest.display()) }
                        }
                        @if p.plan.len() > 5 { div.muted { "…and " (p.plan.len() - 5) " more" } }
                    }
                }
            }

            @for (i, media) in p.item.media.iter().enumerate() {
                (media_card(i, p.item.media.len(), media))
            }

            @if p.child_count > 1 {
                (list_controls(&ListControls {
                    action: &format!("/servers/{id}/items/{}", p.item.rating_key),
                    params: p.params,
                    sorts: CHILD_SORTS,
                    default_sort: if p.item.kind == "show" { "Season order" } else { "Episode order" },
                    title: true,
                    year: false,
                    status: p.child_statuses.is_some(),
                }))
            }
            @if p.child_count > 0 {
                (items_table(id, p.children, p.child_statuses, false, "Nothing matches these filters."))
            }
        },
    )
}

fn duration(ms: u64) -> String {
    let mins = ms / 60_000;
    if mins >= 60 {
        format!("{}h {:02}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
    }
}

fn tag_list(tags: &[Tag], limit: usize) -> String {
    tags.iter()
        .take(limit)
        .map(|t| t.tag.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Summary and facts about the item itself (not its files).
fn about(m: &Metadata) -> Markup {
    let mut facts: Vec<(&str, String)> = Vec::new();
    let mut add = |label: &'static str, value: Option<String>| {
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            facts.push((label, v));
        }
    };
    add("Released", m.originally_available_at.clone());
    add("Runtime", m.duration.filter(|d| *d > 0).map(duration));
    add("Rated", m.content_rating.clone());
    add(
        "Rating",
        match (m.rating, m.audience_rating) {
            (Some(c), Some(a)) => Some(format!("critics {c:.1} · audience {a:.1}")),
            (Some(r), None) | (None, Some(r)) => Some(format!("{r:.1}")),
            _ => None,
        },
    );
    add("Genres", Some(tag_list(&m.genres, 10)));
    add("Directors", Some(tag_list(&m.directors, 5)));
    add("Writers", Some(tag_list(&m.writers, 5)));
    add(
        "Cast",
        Some(
            m.roles
                .iter()
                .take(10)
                .map(|r| match r.role.as_deref().filter(|s| !s.is_empty()) {
                    Some(role) => format!("{} ({role})", r.tag),
                    None => r.tag.clone(),
                })
                .collect::<Vec<_>>()
                .join(", "),
        ),
    );
    add("Studio", m.studio.clone());
    add("Country", Some(tag_list(&m.countries, 5)));
    add("Added", m.added_at.filter(|t| *t > 0).map(unix_date));
    html! {
        @if let Some(tagline) = m.tagline.as_deref().filter(|t| !t.is_empty()) {
            p.muted { em { (tagline) } }
        }
        @if let Some(summary) = m.summary.as_deref().filter(|t| !t.is_empty()) {
            p { (summary) }
        }
        @if !facts.is_empty() {
            dl.facts {
                @for (label, value) in &facts { dt { (label) } dd { (value) } }
            }
        }
    }
}

/// `YYYY-MM-DD` from unix seconds (UTC), without pulling in a date crate.
fn unix_date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn stream_kind(t: u8) -> &'static str {
    match t {
        1 => "Video",
        2 => "Audio",
        3 => "Subtitle",
        _ => "Other",
    }
}

fn stream_summary(s: &Stream) -> String {
    let mut bits: Vec<String> = Vec::new();
    if let Some(t) = s.display_title.as_deref().filter(|t| !t.is_empty()) {
        bits.push(t.to_string());
    }
    match s.stream_type {
        1 => {
            if let (Some(w), Some(h)) = (s.width, s.height) {
                bits.push(format!("{w}×{h}"));
            }
            if let Some(f) = s.frame_rate {
                bits.push(format!("{f:.3} fps").replace(".000", ""));
            }
            if let Some(b) = s.bit_depth {
                bits.push(format!("{b}-bit"));
            }
            if let Some(c) = s.color_space.as_deref() {
                bits.push(c.to_string());
            }
        }
        2 => {
            if let Some(c) = s.channels {
                bits.push(format!("{c} ch"));
            }
        }
        _ => {}
    }
    if s.forced == Some(true) {
        bits.push("forced".into());
    }
    if s.default == Some(true) {
        bits.push("default".into());
    }
    bits.join(" · ")
}

fn media_card(index: usize, count: usize, m: &Media) -> Markup {
    let mut facts: Vec<(&str, String)> = Vec::new();
    let mut add = |label: &'static str, value: Option<String>| {
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            facts.push((label, v));
        }
    };
    add(
        "Resolution",
        match (m.width, m.height, m.video_resolution.as_deref()) {
            (Some(w), Some(h), Some(r)) => Some(format!("{w}×{h} ({r})")),
            (Some(w), Some(h), None) => Some(format!("{w}×{h}")),
            (_, _, r) => r.map(str::to_string),
        },
    );
    add(
        "Video",
        m.video_codec
            .as_ref()
            .map(|c| match m.video_profile.as_deref() {
                Some(p) if !p.is_empty() => format!("{c} ({p})"),
                _ => c.clone(),
            }),
    );
    add("Frame rate", m.video_frame_rate.clone());
    add("Aspect ratio", m.aspect_ratio.map(|a| format!("{a:.2}:1")));
    add(
        "Audio",
        m.audio_codec
            .as_ref()
            .map(|c| match (m.audio_profile.as_deref(), m.audio_channels) {
                (Some(p), Some(ch)) if !p.is_empty() => format!("{c} ({p}) · {ch} ch"),
                (_, Some(ch)) => format!("{c} · {ch} ch"),
                _ => c.clone(),
            }),
    );
    add(
        "Bitrate",
        m.bitrate.map(|b| format!("{:.1} Mbps", b as f64 / 1000.0)),
    );
    add("Container", m.container.clone());
    add("Duration", m.duration.filter(|d| *d > 0).map(duration));
    let total: u64 = m.parts.iter().filter_map(|p| p.size).sum();
    add("Size", (total > 0).then(|| format_size(total)));
    html! {
        div.card {
            h2 {
                @if count > 1 { "Version " (index + 1) " of " (count) " · " (m.summary()) }
                @else { "File" }
            }
            @if !facts.is_empty() {
                dl.facts {
                    @for (label, value) in &facts { dt { (label) } dd { (value) } }
                }
            }
            @for (i, part) in m.parts.iter().enumerate() {
                (part_block(i, m.parts.len(), part))
            }
        }
    }
}

fn part_block(index: usize, count: usize, p: &Part) -> Markup {
    html! {
        @if count > 1 { h3 { "Part " (index + 1) " of " (count) } }
        @if let Some(file) = p.file.as_deref() {
            div.muted { "On server" }
            div.path { (file) }
        }
        @if let Some(size) = p.size { p.muted { (format_size(size)) } }
        @if !p.streams.is_empty() {
            table.streams {
                thead { tr { th { "Stream" } th { "Codec" } th { "Language" } th { "Details" } th { "Bitrate" } } }
                tbody {
                    @for s in &p.streams {
                        tr {
                            td { (stream_kind(s.stream_type)) }
                            td { (s.codec.as_deref().unwrap_or("")) @if let Some(pr) = s.profile.as_deref().filter(|p| !p.is_empty()) { " " span.muted { "(" (pr) ")" } } }
                            td { (s.language.as_deref().unwrap_or("")) }
                            td { (stream_summary(s)) }
                            td { @if let Some(b) = s.bitrate { (b) " kbps" } }
                        }
                    }
                }
            }
        }
    }
}

/// One server's results, with each item's status when comparing.
pub type ServerResults = (Resource, Vec<Metadata>, Option<Vec<Option<Status>>>);

pub struct SearchResults<'a> {
    pub found: &'a [ServerResults],
    /// Names of servers that could not be searched.
    pub skipped: &'a [String],
}

/// Where a search looks: everywhere, one server, or one library of a server.
#[derive(Clone, Copy, Default)]
pub struct SearchScope<'a> {
    pub server: Option<&'a str>,
    pub library: Option<&'a str>,
    /// Search your own servers too, when searching everywhere.
    pub include_mine: bool,
}

/// Search box that swaps just the page's `<main>` and shows a spinner meanwhile.
/// With `params`, it also offers type and sort menus for the results.
/// `status` adds the menu comparing with your library.
pub fn search_form(
    scope: SearchScope<'_>,
    query: &str,
    placeholder: &str,
    params: Option<&ListParams>,
    status: bool,
) -> Markup {
    html! {
        form.row.search method="get" action="/search"
            hx-get="/search" hx-target="main" hx-select="main" hx-swap="outerHTML"
            hx-push-url="true" hx-indicator="#search-spinner" hx-disabled-elt="find button"
            data-autosubmit {
            @if let Some(server) = scope.server { input type="hidden" name="server" value=(server); }
            @if let Some(library) = scope.library { input type="hidden" name="library" value=(library); }
            @if scope.include_mine { input type="hidden" name="mine" value="1"; }
            input type="search" name="q" value=(query) placeholder=(placeholder);
            @if let Some(params) = params {
                (kind_select(params))
                (sort_select(SEARCH_SORTS, "Best match", &params.sort_value()))
                @if status { (status_select(params)) }
            }
            button.primary type="submit" { "Search" }
            span.spinner #search-spinner role="status" aria-label="Searching" {}
        }
    }
}

pub struct SearchPage<'a> {
    pub user: &'a str,
    pub query: &'a str,
    pub scope: SearchScope<'a>,
    /// Server and library names, for the heading and breadcrumbs.
    pub server: Option<&'a Resource>,
    pub library_title: Option<&'a str>,
    pub params: &'a ListParams,
    pub results: Option<SearchResults<'a>>,
    pub mine: &'a Mine,
    /// How many of your own servers were left out of an everywhere search.
    pub hidden_mine: usize,
}

/// The same search, including your own servers.
fn include_mine_link(query: &str, params: &ListParams) -> String {
    let mut pairs = vec![("q", query.to_string())];
    pairs.extend(params.pairs());
    pairs.push(("mine", "1".into()));
    href("/search", &pairs)
}

pub fn search_page(p: &SearchPage<'_>) -> Markup {
    let place = match (p.library_title, p.server) {
        (Some(lib), _) => format!("Search {lib}"),
        (None, Some(server)) => format!("Search {}", server.name),
        _ => "Search all servers".to_string(),
    };
    layout(
        "Search",
        "servers",
        Some(p.user),
        html! {
            div.crumbs {
                a href="/servers" { "Servers" }
                @if let Some(server) = p.server {
                    " › " a href={"/servers/" (server.client_identifier)} { (server.name) }
                    @if let (Some(lib), Some(key)) = (p.library_title, p.scope.library) {
                        " › " a href={"/servers/" (server.client_identifier) "/libraries/" (key)} { (lib) }
                    }
                }
            }
            h1 { "Search" }
            (search_form(p.scope, p.query, &place, Some(p.params), !matches!(p.mine, Mine::Off)))
            @if let Some(results) = &p.results {
                (mine_note(p.mine))
                @for name in results.skipped { p.muted { "Skipped " (name) " (unreachable)" } }
                @if results.found.iter().all(|(_, items, _)| items.is_empty()) {
                    p.empty {
                        @if let Some(kind) = kind_label(p.params) {
                            "No " (kind.to_lowercase()) " found for “" (p.query) "”."
                        } @else {
                            "No results for “" (p.query) "”."
                        }
                    }
                }
                @for (server, items, statuses) in results.found {
                    @if !items.is_empty() {
                        @if p.scope.server.is_none() { h2 { (server.name) } }
                        (items_table(&server.client_identifier, items, statuses.as_deref(), true, ""))
                    }
                }
                @if p.hidden_mine > 0 {
                    p.muted {
                        "Your own " @if p.hidden_mine == 1 { "server wasn't" } @else { "servers weren't" } " searched. "
                        a href=(include_mine_link(p.query, p.params)) { "Search them too" }
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
                    JobState::Waiting { reason, retry_at } => {
                        @let secs = retry_at.saturating_duration_since(std::time::Instant::now()).as_secs();
                        span.warn { "Waiting for server" }
                        span.muted {
                            @if secs > 0 { " · retrying in " (format_wait(secs)) } @else { " · retrying now" }
                        }
                        div.muted.small { (reason) }
                    }
                    JobState::Paused => { span.muted { "Paused" } }
                    JobState::Failed(message) => { span.bad { (message) } }
                }
            }
            td.actions {
                @if job.state.can_pause() {
                    form.inline method="post" action={"/downloads/" (job.id) "/pause"} {
                        button.link type="submit" { "Pause" }
                    }
                }
                @if job.state.can_resume() {
                    form.inline method="post" action={"/downloads/" (job.id) "/resume"} {
                        button.link type="submit" {
                            @if matches!(job.state, JobState::Failed(_)) { "Retry" } @else { "Resume" }
                        }
                    }
                }
                @if job.state.is_active() {
                    form.inline method="post" action={"/downloads/" (job.id) "/cancel"} {
                        button.link type="submit" { "Cancel" }
                    }
                }
            }
        }
    }
}

/// `90` → `1m 30s`.
fn format_wait(secs: u64) -> String {
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s}s"),
        (m, 0) => format!("{m}m"),
        (m, s) => format!("{m}m {s}s"),
    }
}

/// Pause all / resume all, for the whole queue and, when it holds jobs from
/// several servers, for each server.
fn queue_actions(jobs: &[Job]) -> Markup {
    let mut servers: Vec<(&str, &str)> = Vec::new();
    for job in jobs.iter().filter(|j| j.state.is_active()) {
        if !servers.iter().any(|(id, _)| *id == job.server_id) {
            servers.push((&job.server_id, &job.server));
        }
    }
    let buttons = |server: Option<&str>| {
        let of = |j: &&Job| server.is_none_or(|s| j.server_id == s);
        let pausable = jobs.iter().filter(of).any(|j| j.state.can_pause());
        let paused = jobs
            .iter()
            .filter(of)
            .any(|j| matches!(j.state, JobState::Paused));
        html! {
            @if pausable {
                form.inline method="post" action="/downloads/pause" {
                    @if let Some(s) = server { input type="hidden" name="server" value=(s); }
                    button type="submit" { "Pause all" }
                }
            }
            @if paused {
                form.inline method="post" action="/downloads/resume" {
                    @if let Some(s) = server { input type="hidden" name="server" value=(s); }
                    button type="submit" { "Resume all" }
                }
            }
        }
    };
    html! {
        @if !servers.is_empty() {
            div.queue-actions {
                div.row { (buttons(None)) }
                @if servers.len() > 1 {
                    @for (id, name) in &servers {
                        div.row { span.muted { (name) } (buttons(Some(id))) }
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
                (queue_actions(jobs))
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

# plex-grabber plan

A slim Rust replacement for [Reaparr](https://github.com/Reaparr/Reaparr): sign in
with a Plex account, browse/search the servers you have access to, and download
media to disk in a clean, Plex-friendly layout.

## Decisions

| Topic | Decision |
|---|---|
| Language | Rust (tokio, reqwest/rustls, serde, clap) |
| Auth | Plex PIN flow, no password handling |
| Token storage | Plain JSON config file, `0600` on Unix |
| Download scope | Single files, whole seasons, whole shows |
| File layout | Renamed into a Plex-style layout (below) |
| Owner download setting | Respected: clear error when the owner disallows downloads |
| Web UI | Phase 2; single user; axum + htmx; downloads saved to the server's disk |
| Distribution | CLI release builds for Linux/macOS/Windows; Docker image |

## Workspace

```
crates/
  plex-core/   library: config, plex.tv client, server client, naming, downloader
  plex-cli/    `plex-grabber` binary (clap)
  plex-web/    phase 2: axum + htmx (reuses plex-core)
```

## Configuration

- Config dir: `$PLEX_GRABBER_CONFIG_DIR`, else the OS config dir
  (`~/.config/plex-grabber` on Linux). Docker sets it to `/config`.
- `config.json` holds a persistent `client_id` (generated once) and the auth
  token/username after login.
- Movies and TV shows go to separate folders, so a container can map in an
  existing media library for each:
  - `--movies-dir` / `$PLEX_GRABBER_MOVIES_DIR` (Docker: `/movies`)
  - `--tv-dir` / `$PLEX_GRABBER_TV_DIR` (Docker: `/tv`)
  - If unset, the CLI falls back to `./downloads/Movies` and `./downloads/TV Shows`.

## Plex API

- **PIN login**: `POST https://plex.tv/api/v2/pins?strong=true` → show
  `https://app.plex.tv/auth#?clientID=…&code=…` → poll
  `GET /api/v2/pins/{id}` until `authToken` is set.
- **User**: `GET https://plex.tv/api/v2/user`.
- **Servers**: `GET https://plex.tv/api/v2/resources?includeHttps=1&includeRelay=1`,
  keep `provides` containing `server`. Each has its own `accessToken` and a list
  of connections. Probe in parallel (`GET /identity`), prefer
  local → remote → relay, cache the winner.
- **Browse**: `/library/sections`, `/library/sections/{id}/all`,
  `/library/metadata/{key}`, `/library/metadata/{key}/children`,
  `/library/metadata/{key}/allLeaves` (all episodes of a show).
- **Search**: `/hubs/search?query=…`.
- **Download**: `Media[].Part[].key` + `?download=1`, streamed to
  `<name>.part`, resumed with HTTP `Range`, renamed on completion.

All requests send `Accept: application/json` and the standard `X-Plex-*` headers.

## File layout

```
<movies-dir>/{Title} ({Year})/{Title} ({Year}).{ext}
<tv-dir>/{Show} ({Year})/Season {NN}/{Show} - S{NN}E{NN} - {Episode Title}.{ext}
```

- Specials go in `Season 00`.
- Multi-part media gets ` - pt{N}` before the extension.
- Names are sanitized for Windows (`<>:"/\|?*`, trailing dots/spaces, reserved names).

## Phase 1: CLI

1. Workspace skeleton, config storage, plex.tv client types. ✅
2. `login`, `logout`, `whoami`. ✅
3. `servers`, with connection probing. ✅
4. `libraries <server>`, `ls <server> <library|key>`. ✅
5. `search "<query>" [--server]`.
6. `download <server> <key>`: single file, progress bar, resume, layout naming.
7. Season/show downloads, `--parallel N`.
8. Polish: `--json`, error messages, GitHub Actions release builds
   (linux x86_64/aarch64, macOS x86_64/aarch64, windows x86_64), Dockerfile + GHCR
   (volumes: `/config`, `/movies`, `/tv`).

## Phase 2: Web

- axum server (`plex-grabber serve`, or a separate `plex-web` binary).
- PIN login in the browser; token stored in the same config file.
- Pages: servers → libraries → items, plus search.
- "Download" button enqueues to an in-memory queue; progress via htmx polling/SSE.
- Single user: optional basic-auth/shared password in front of the UI.

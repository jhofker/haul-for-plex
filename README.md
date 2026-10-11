# plex-grabber

A small tool for downloading media from the Plex servers your account can
access, saved into a clean Plex-style folder layout. Use it from the command
line or through a simple web interface.

- Sign in with your Plex account (no password handling; you approve it on plex.tv)
- List and search every server you have access to, owned or shared
- Download movies, episodes, whole seasons or whole shows
- Interrupted downloads resume where they left off
- Files are renamed into a layout Plex, Jellyfin and friends understand
- One static binary, or a ~40 MB Docker image

## Install

**Prebuilt binary:** download the archive for your platform from the
[releases page](https://github.com/jhofker/plex-grabber/releases) (Linux,
macOS and Windows).

**From source:** `cargo install --path crates/plex-cli` (installs `plex-grabber`).

**Docker:** `docker pull ghcr.io/jhofker/plex-grabber` (`latest` is the newest
release; pin `0.1` or `0.1.0` to stay on a version)

## Command line

```console
$ plex-grabber login                       # prints a link; approve it in your browser
$ plex-grabber servers                     # your servers and how each is reachable
$ plex-grabber libraries "Home"            # server names can be abbreviated
$ plex-grabber ls "Home" movies            # a library, or any item key
$ plex-grabber search "blade runner"       # across all your servers
$ plex-grabber download "Home" 84521       # a movie, episode, season or show key
$ plex-grabber download "Home" 100583 --parallel 4 --dry-run
$ plex-grabber download "Home" 84521 --version 4k   # pick a version
```

Add `--json` to any command for machine-readable output. Keys come from `ls`
and `search`.

Run `plex-grabber download --help` for all options. Existing files of the right
size are skipped; a file of a different size is only replaced with `--force`.

## Where files go

```text
<movies-dir>/The Matrix (1999)/The Matrix (1999).mkv
<tv-dir>/The Office (US) (2005)/Season 06/The Office (US) - S06E16 - The Manager and the Salesman.mkv
```

Set the folders with `--movies-dir` / `--tv-dir` or the environment variables
below. Without them, files go under `./downloads/Movies` and
`./downloads/TV Shows`. Downloads are written as `<name>.part` and renamed when
complete, so a half-finished file never looks finished.

## Web interface

```console
$ plex-grabber serve                       # http://127.0.0.1:8080
$ plex-grabber serve --bind 0.0.0.0:8080 --password 'something long'
```

Sign in with Plex in the browser, then browse servers and libraries, search,
and press **Download**. The Downloads page shows live progress and lets you
cancel. It is designed for a single user; the queue lives in memory, so
re-queue anything unfinished after a restart (partial files resume).

If you listen beyond localhost, set a password. It is asked for with HTTP basic
auth (any username). Do not expose the port to the internet without a reverse
proxy providing HTTPS.

## Docker

```yaml
services:
  plex-grabber:
    image: ghcr.io/jhofker/plex-grabber:latest
    ports: ["8080:8080"]
    environment:
      PLEX_GRABBER_WEB_PASSWORD: change-me
    volumes:
      - ./config:/config        # sign-in and settings
      - /path/to/movies:/movies # your movies folder
      - /path/to/tv:/tv         # your TV folder
    # user: "1000:1000"         # write files as this user instead of root
```

Open <http://localhost:8080> and sign in. To use the CLI in the image:
`docker compose run --rm plex-grabber servers`.

## Configuration

| Environment variable | Meaning | Default (Docker) |
|---|---|---|
| `PLEX_GRABBER_CONFIG_DIR` | where `config.json` (token, client id) lives | OS config dir (`/config`) |
| `PLEX_GRABBER_MOVIES_DIR` | movies folder | `./downloads/Movies` (`/movies`) |
| `PLEX_GRABBER_TV_DIR` | TV folder | `./downloads/TV Shows` (`/tv`) |
| `PLEX_GRABBER_BIND` | web interface address | `127.0.0.1:8080` (`0.0.0.0:8080`) |
| `PLEX_GRABBER_WEB_PASSWORD` | web interface password | none |

The config file holds your Plex token and is only readable by its owner.

## Notes

- A server owner can disable downloads for people they share with. Those
  servers refuse the download and plex-grabber reports it; it does not try to
  work around the setting.
- When an item has several versions (for example 1080p and 4K), the first is
  used unless you choose: `--version 2` (numbered as in `ls`) or
  `--version 4k` / `1080p` / `720p` / `sd`. For a season or show, episodes that
  lack the requested version use their first one. Versions other than the first
  are saved as `Movie (2023) - 4K.mkv` so both can sit side by side. In the web
  interface a movie or episode with several versions has a download button per
  version, and seasons and shows have a version picker.

## Development

```console
$ cargo test --workspace
$ cargo clippy --workspace --all-targets -- -D warnings
```

The workspace has three crates: `plex-core` (Plex clients, naming, downloader),
`plex-cli` (the `plex-grabber` binary) and `plex-web` (the web interface).
See `PLAN.md` for the design notes.

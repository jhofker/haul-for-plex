FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --release --locked -p plex-cli

FROM gcr.io/distroless/cc-debian12
COPY --from=build /src/target/release/plex-grabber /usr/local/bin/plex-grabber

# Settings and Plex sign-in live in /config; map your libraries into /movies and /tv.
ENV PLEX_GRABBER_CONFIG_DIR=/config \
    PLEX_GRABBER_MOVIES_DIR=/movies \
    PLEX_GRABBER_TV_DIR=/tv
VOLUME ["/config", "/movies", "/tv"]

ENTRYPOINT ["/usr/local/bin/plex-grabber"]
CMD ["--help"]

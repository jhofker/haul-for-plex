FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --release --locked -p haul-for-plex

FROM gcr.io/distroless/cc-debian12
COPY --from=build /src/target/release/haul /usr/local/bin/haul

# Settings and Plex sign-in live in /config; map your libraries into /movies and /tv.
ENV HAUL_CONFIG_DIR=/config \
    HAUL_MOVIES_DIR=/movies \
    HAUL_TV_DIR=/tv
VOLUME ["/config", "/movies", "/tv"]

# The web interface; run other commands with `docker run ... haul <command>`.
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/haul"]
CMD ["serve", "--bind", "0.0.0.0:8080"]

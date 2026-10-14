# cargo-chef builds the dependencies in their own layer, so a source-only change
# doesn't recompile the ~300 crates they pull in.
FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm AS chef
WORKDIR /src

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked -p haul-for-plex --recipe-path recipe.json
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

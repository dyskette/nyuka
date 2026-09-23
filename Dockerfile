# syntax=docker/dockerfile:1

# Stage order is fixed: the Rust build embeds web/dist, so the frontend must
# be built first (ADR-0006).
FROM node:22-alpine AS web
WORKDIR /web
COPY web/package*.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:1.98-slim AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends \
      pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
# Cargo.lock must be committed: this is a binary, so the lockfile is the
# reproducibility guarantee, and this COPY fails without it. Generate it with
# `cargo generate-lockfile` on first checkout.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
# Every workspace member's manifest has to exist, even the ones this image does
# not build: Cargo reads the whole workspace before it resolves `-p nyuka-api`,
# and a missing member fails with `failed to load manifest for workspace
# member`. `xtask` is a build tool and contributes nothing to the binary — it
# is here so the workspace resolves, and copied before `crates/` because it
# changes far less often, so it stays out of the layer that rebuilds.
COPY xtask/ xtask/
COPY crates/ crates/
COPY --from=web /web/dist web/dist
COPY web/openapi.json web/openapi.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p nyuka-api && \
    cp target/release/nyuka-api /nyuka-api

FROM gcr.io/distroless/cc-debian12
COPY --from=build /nyuka-api /nyuka-api
USER nonroot:nonroot
EXPOSE 8080
ENTRYPOINT ["/nyuka-api"]

# calendry-solver — the service image.
#
# WHY THIS FILE MOVED HERE. It used to live in the calendry repo as
# `.config/Dockerfile.solver`, on the reasoning that one place should describe
# the whole stack. That held while nothing but calendry's dev compose built it.
# It stopped holding the moment this repository needed to publish its own image:
# a workflow here cannot reach into another repository for its Dockerfile, so
# the alternative was a second copy — two build definitions for one binary,
# which is the "two implementations of one concept" drift that has already bitten
# this project three times (paramField, weekCountOf, blockTime).
#
# The one-place property survives, because calendry vendors this repo as a git
# submodule: its `docker-compose.dev.yml` builds `vendor/calendry-solver` with
# this file, so `docker compose up` still brings up app, database and solver
# from a single checkout. There is exactly one Dockerfile and both consumers
# read it.
#
# Build context is this repository's root. `.dockerignore` keeps `target/` out —
# 1.4 GB of Rust build artifacts that would otherwise be uploaded on every
# build.

# rust-version = "1.90" and edition 2024 in the workspace manifest, so the
# toolchain floor is real rather than incidental.
FROM rust:1.90-slim-bookworm AS build

WORKDIR /src

# protoc is a build dependency: calendry-proto arrives as a git submodule of the
# solver and is compiled by build.rs, not fetched as a prebuilt crate.
RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Manifests first, so the dependency compile is cached and only re-runs when the
# manifests actually change. A source edit then rebuilds just the workspace
# crates rather than the entire dependency graph.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY vendor ./vendor

RUN cargo build --release --bin calendry-solver \
    && strip target/release/calendry-solver

# ---------------------------------------------------------------------------

FROM debian:bookworm-slim AS runtime

# ca-certificates only; the solver speaks gRPC over plain TCP inside the compose
# network and needs nothing else at runtime.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home solver

COPY --from=build /src/target/release/calendry-solver /usr/local/bin/calendry-solver

USER solver

# 0.0.0.0, not 127.0.0.1: the solver's default binds to its own loopback, which
# inside a container is reachable by nothing. This is the whole reason a
# host-run solver was unreachable from the app container.
ENV CALENDRY_SOLVER_ADDR=0.0.0.0:50051

EXPOSE 50051

ENTRYPOINT ["/usr/local/bin/calendry-solver"]

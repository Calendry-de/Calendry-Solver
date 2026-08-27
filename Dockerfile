# calendry-solver — the service image.
#
# Lives here rather than in calendry's .config/ because this repo publishes its
# own image and a workflow cannot reach into another repo for a Dockerfile. The
# calendry dev compose builds ./vendor/calendry-solver with this file, so there
# is still one definition and one checkout.
#
# Context is this repo's root; .dockerignore keeps target/ out.

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

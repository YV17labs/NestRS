# syntax=docker/dockerfile:1.7
#
# Multi-stage build for the entire workspace.
#
# Every binary crate under `apps/` is compiled in the builder and shipped in
# the runtime image. Which one runs is chosen at `docker run` time by
# overriding the entrypoint:
#
#   docker run --rm -p 3000:3000 nestrs                # runs the default (app)
#   docker run --rm -p 3000:3000 nestrs /usr/local/bin/<other-app>
#
# Stages:
#   1. chef    — rust toolchain + cargo-chef, shared by planner & builder
#   2. planner — produces a recipe describing the workspace dependency graph
#   3. builder — cooks deps (cacheable), then compiles every workspace binary
#   4. runtime — distroless, non-root, minimal attack surface
#
# Security choices:
#   - Runtime image is `gcr.io/distroless/cc-debian13:nonroot`: no shell, no
#     package manager, runs as UID 65532 (nonroot) by default.
#   - Rust version and cargo-chef version are pinned (override via --build-arg).
#   - Only the release binaries land in the runtime — sources, target/, and
#     build tooling are dropped between stages.
#   - No HEALTHCHECK directive: rely on Kubernetes liveness/readiness probes
#     exposed at /health/{live,ready,startup}.

ARG RUST_VERSION=1.95
ARG CARGO_CHEF_VERSION=0.1.77

# --- chef ---------------------------------------------------------------------
FROM rust:${RUST_VERSION}-slim-trixie AS chef
ARG CARGO_CHEF_VERSION
RUN cargo install cargo-chef --locked --version ${CARGO_CHEF_VERSION}
WORKDIR /app

# --- planner ------------------------------------------------------------------
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# --- builder ------------------------------------------------------------------
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --workspace --bins
# Collect every workspace binary into a single dir so the runtime stage can
# copy them without enumerating each one.
RUN mkdir -p /out && \
    find target/release -maxdepth 1 -type f -executable -exec cp {} /out/ \;

# --- runtime ------------------------------------------------------------------
FROM gcr.io/distroless/cc-debian13:nonroot AS runtime

LABEL org.opencontainers.image.title="NestRS"
LABEL org.opencontainers.image.description="Applications built on the NestRS framework"
LABEL org.opencontainers.image.url="https://nestrs.dev"
LABEL org.opencontainers.image.documentation="https://nestrs.dev"
LABEL org.opencontainers.image.source="https://github.com/NestRS/NestRS"
LABEL org.opencontainers.image.licenses="MIT"

COPY --from=builder --chown=nonroot:nonroot /out/ /usr/local/bin/

EXPOSE 3000
USER nonroot:nonroot
# Default app — override at runtime: `docker run ... /usr/local/bin/<app>`
ENTRYPOINT ["/usr/local/bin/app"]

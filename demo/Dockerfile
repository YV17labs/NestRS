# syntax=docker/dockerfile:1.7
# Build from the repository root: the demo reaches the framework through ../crates.
#   docker build -f demo/Dockerfile -t nestrs .
#   docker run --rm -p 3001:3001 nestrs /usr/local/bin/auth   # default: api

ARG RUST_VERSION=1.97
ARG CARGO_CHEF_VERSION=0.1.77

# --- chef ---------------------------------------------------------------------
FROM rust:${RUST_VERSION}-slim-trixie AS chef
ARG CARGO_CHEF_VERSION
RUN cargo install cargo-chef --locked --version ${CARGO_CHEF_VERSION}
WORKDIR /app

# --- planner ------------------------------------------------------------------
FROM chef AS planner
COPY . .
WORKDIR /app/demo
RUN cargo chef prepare --recipe-path recipe.json

# --- builder ------------------------------------------------------------------
FROM chef AS builder
COPY --from=planner /app/demo/recipe.json /app/demo/recipe.json
WORKDIR /app/demo
RUN cargo chef cook --release --recipe-path recipe.json
COPY . /app
RUN cargo build --release --workspace --exclude seed --bins
RUN mkdir -p /out && \
    find target/release -maxdepth 1 -type f -executable -exec cp {} /out/ \;

# --- runtime ------------------------------------------------------------------
FROM gcr.io/distroless/cc-debian13:nonroot AS runtime

LABEL org.opencontainers.image.title="NestRS"
LABEL org.opencontainers.image.description="Applications built on the NestRS framework"
LABEL org.opencontainers.image.url="https://nestrs.dev"
LABEL org.opencontainers.image.documentation="https://nestrs.dev"
LABEL org.opencontainers.image.source="https://github.com/YV17labs/NestRS"
LABEL org.opencontainers.image.licenses="MIT"

COPY --from=builder --chown=nonroot:nonroot /out/ /usr/local/bin/

EXPOSE 3002
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/api"]

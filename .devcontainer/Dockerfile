# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.96
ARG NODE_MAJOR=24

FROM mcr.microsoft.com/devcontainers/base:trixie AS dev

ARG RUST_VERSION
ARG NODE_MAJOR

RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      mold build-essential cmake clang perl pkg-config libssl-dev curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*

RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends gnupg \
 && mkdir -p /etc/apt/keyrings \
 && curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key \
      | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg \
 && printf 'Types: deb\nURIs: https://deb.nodesource.com/node_%s.x/\nSuites: nodistro\nComponents: main\nSigned-By: /etc/apt/keyrings/nodesource.gpg\n' "${NODE_MAJOR}" \
      > /etc/apt/sources.list.d/nodesource.sources \
 && apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends nodejs \
 && rm -rf /var/lib/apt/lists/*

USER vscode

ENV PATH=/home/vscode/.cargo/bin:${PATH}

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal \
          --default-toolchain "${RUST_VERSION}" \
          -c rustfmt -c clippy -c rust-analyzer -c llvm-tools-preview \
 && cargo install --locked cargo-binstall \
 && cargo binstall --no-confirm --locked \
      just bacon cargo-nextest cargo-llvm-cov cargo-audit cargo-outdated

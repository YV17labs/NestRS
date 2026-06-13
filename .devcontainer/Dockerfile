# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.96

FROM mcr.microsoft.com/devcontainers/base:trixie AS dev

ARG RUST_VERSION

RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      mold build-essential cmake clang perl pkg-config libssl-dev curl ca-certificates \
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

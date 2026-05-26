# syntax=docker/dockerfile:1

FROM mcr.microsoft.com/devcontainers/rust:1-trixie AS dev

# mold: a fast linker — the biggest win on incremental dev rebuilds.
# Wired up as the Linux linker in .cargo/config.toml.
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends mold \
 && rm -rf /var/lib/apt/lists/*

USER vscode

RUN rustup toolchain install stable \
      --component rustfmt --component clippy --component rust-analyzer --component llvm-tools-preview \
 && rustup default stable \
 && cargo install --locked cargo-binstall \
 && cargo binstall --no-confirm --locked \
      just bacon cargo-nextest cargo-llvm-cov cargo-audit cargo-outdated

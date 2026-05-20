# syntax=docker/dockerfile:1
#
# Devcontainer aligned with the production runtime:
#   - Debian 13 (Trixie) + glibc — same base as `gcr.io/distroless/cc-debian13`
#     used by the root Dockerfile, so dev-built binaries are ABI-compatible
#     with what ships to prod.
#   - Microsoft's official Rust devcontainer image: rustup + cargo + clippy +
#     rustfmt are already inside; we add rust-analyzer plus the workflow tools
#     the README lists, so a fresh devcontainer is ready to go with no manual
#     `cargo install` step.

FROM mcr.microsoft.com/devcontainers/rust:1-trixie AS dev

USER vscode

# cargo-binstall fetches prebuilt binaries instead of compiling each tool from
# source — ~30 s of download vs ~10 min of codegen.
RUN rustup component add rust-analyzer llvm-tools-preview \
 && cargo install --locked cargo-binstall \
 && cargo binstall --no-confirm --locked \
      just bacon cargo-nextest cargo-llvm-cov

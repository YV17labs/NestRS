//! Integration tests mirroring `src/` (see CLAUDE.md) — one binary, one module per concern.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod context;
mod diagnostics;
mod duplicate_operation;
mod federation;
mod global_pipe;
mod guard;
mod layer_pool;
mod limits;
mod loader;
mod operation;
mod pipe;
mod read_only;
mod redact;
mod resolver;
mod scope;
mod sdl_snapshot;
mod subscription;

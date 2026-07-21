//! Integration tests mirroring `src/` (see CLAUDE.md).
//!
//! Transport-binding tests are gated on the same feature that exposes them in
//! `src/`: run with `cargo test -p nest-rs-authz --features full` to exercise
//! every bridge in this crate.

mod ability;
mod builder;

#[cfg(feature = "http")]
mod http;

#[cfg(feature = "graphql")]
mod graphql;

#[cfg(feature = "mcp")]
mod mcp;

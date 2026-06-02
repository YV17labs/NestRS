//! Integration tests mirroring `src/` (see CLAUDE.md).
//!
//! Transport-binding tests are gated on the same feature that exposes them in
//! `src/`: run with `cargo test -p nestrs-authz --features full` to exercise
//! every bridge in this crate.

mod ability;

#[cfg(feature = "http")]
mod http;

#[cfg(feature = "graphql")]
mod graphql;

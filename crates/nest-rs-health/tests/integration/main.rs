//! Integration tests mirroring `src/`; `src/config.rs` is covered in-file and
//! through the boot in `module`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]
mod controller;
mod indicator;
mod module;
mod service;

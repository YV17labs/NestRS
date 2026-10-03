//! Structural checks over the repository's paths and manifests — the naming
//! law, the test-target layout, and the names written outside Rust.
//!
//! What a check here reads is a file path, a manifest, or a constant a crate
//! declares; it never proves what code *does*. That is held a rung up — by
//! types, by `clippy.toml`'s resolved-path lints, and by behaviour tests — where
//! a renamed import or a macro cannot hide a member.
//!
//! `src/` carries only what the checks share: where to read, and how a
//! baseline behaves. A check itself is always a test, so it lives in `tests/`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a check fails the suite by panicking, as the test it runs in would"
)]

pub mod baseline;
pub mod imports;
pub mod sources;

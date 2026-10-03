//! Integration tests for `nest-rs-pipes`, mirroring `src/` (see CLAUDE.md): each
//! pipe through `Pipe::transform`, the call every transport's binding makes, in
//! process.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod pipes;

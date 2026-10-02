//! `nestrs` CLI integration suite — drives the built binary against a scratch
//! workspace on disk. No live infrastructure, so it is the `integration` suite.
//!
//! The module tree mirrors `src/`: one file per command, `generate/` per
//! generator, and the shared fixtures in [`harness`].
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod cli;
mod doctor;
mod generate;
mod harness;
mod info;
mod lint;
mod new;

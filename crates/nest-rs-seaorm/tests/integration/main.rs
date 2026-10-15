//! `nest-rs-seaorm`'s in-process suite: what the exposure decorators emit, in
//! `expose/`, which mirrors their macro crate's `src/` — their refusals are
//! trybuild snapshots in `nest-rs-macro-hygiene`'s suite — and the pool's
//! refusal of a server it cannot verify, before any login.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod error;
mod expose;
mod module;

/// The principal the suite's authentication stand-ins attach: a caller with no
/// identity of its own.
#[derive(Clone, Debug, Default)]
pub struct Anyone;

impl nest_rs_core::AmbientValue for Anyone {}

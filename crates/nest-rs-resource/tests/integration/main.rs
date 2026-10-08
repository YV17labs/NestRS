//! What the exposure decorators emit; their refusals are trybuild snapshots in
//! `nest-rs-macro-hygiene`'s suite.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod exposures;
#[cfg(feature = "graphql")]
mod graphql;
mod wire_enum;

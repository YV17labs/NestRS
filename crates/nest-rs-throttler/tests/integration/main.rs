//! Public-API exercise for `Throttle` + `InMemoryThrottler` keyed by client.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod guard;
mod module;
mod store;
mod wiring;

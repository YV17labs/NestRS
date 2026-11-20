//! The public API of `nest-rs-resilience`, driven on a paused clock.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a suite fails by panicking; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod bulkhead;
mod policy;

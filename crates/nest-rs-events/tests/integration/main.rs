//! Integration suite root for `nest-rs-events`. Every test lives in the
//! module named for the `src/` concern it covers: [`bus`] for emission
//! through the discovered `#[on_event]` methods, [`order`] for the
//! deterministic dispatch-order guarantee.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod bus;
mod order;

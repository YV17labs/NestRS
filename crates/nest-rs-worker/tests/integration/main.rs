//! Integration suite root for `nest-rs-worker`. Every test lives in the
//! module named for the `src/` concern it covers: [`context`] for the
//! [`nest_rs_worker::JobContext`] contract through `run_in_job_context`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod context;

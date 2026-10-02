//! In-process integration suite root for `nest-rs-redis` — no Redis. Every
//! test lives in the module named for the `src/` concern it covers:
//! [`connection`] for the connection's budget against the nets above it,
//! [`queue`] for the producer binding's composition, [`schedule`] for the
//! occurrence-lock binding's, [`worker`] for what the consumer transport refuses
//! at boot and the panic backstop it wires.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod connection;
mod queue;
mod schedule;
mod worker;

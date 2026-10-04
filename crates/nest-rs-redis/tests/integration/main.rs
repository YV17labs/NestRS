//! `nest-rs-redis`'s suite in process, without a Redis: [`connection`] for the
//! boot against a scripted server and the budget's place below the ports' nets,
//! [`tls`] for a certificate refused at the handshake, [`queue`] and
//! [`schedule`] for what their bindings' composition refuses before Redis is
//! dialled, and [`worker`] for what `RedisWorker` refuses before its storage is
//! opened. What only a live Redis shows is in `e2e`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod connection;
#[path = "../harness/mod.rs"]
mod harness;
mod queue;
mod schedule;
mod tls;
mod worker;

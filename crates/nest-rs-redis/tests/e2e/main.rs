//! Live-Redis e2e for `nest-rs-redis`. One module per concern in `src/`:
//! [`throttler`] for the cross-process rate-limit store, [`concurrency`] and
//! [`replicas`] for the worker's fetch guarantees, [`correlation`] for the
//! trace context that crosses the producer/consumer process boundary, and
//! [`portable_producer`] for the two names `QueueModule::for_root` binds.
//!
//! Needs a reachable Redis — gated out of `unit` by the nextest `binary(e2e)`
//! filter, and behind the `throttler` feature (off by default, so producer /
//! consumer apps that never rate-limit pull neither `redis` nor
//! `nest-rs-throttler`). Run it explicitly:
//!
//! ```bash
//! cargo nextest run -p nest-rs-redis --features throttler -E 'binary(e2e)'
//! ```
//!
//! The URL comes from `NESTRS_QUEUE__URL` (the dev container wires
//! `redis://redis:6379`); unset, it falls back to that default. This file holds
//! the suite's shared fixtures and nothing else — every test lives in the
//! module named for the concern it covers.

mod concurrency;
mod correlation;
mod portable_producer;
mod replicas;
mod throttler;

use std::time::{SystemTime, UNIX_EPOCH};

use nest_rs_redis::QueueConnection;

fn redis_url() -> String {
    std::env::var("NESTRS_QUEUE__URL").unwrap_or_else(|_| "redis://redis:6379".to_string())
}

/// A key unique to this process, call site and wall-clock instant, so a rerun
/// (or a recycled PID) never inherits a prior run's still-live window.
fn unique_key(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("redis-q2:{tag}:{}:{nanos}", std::process::id())
}

async fn connect() -> QueueConnection {
    QueueConnection::connect(&redis_url())
        .await
        .expect("connect to the dev container Redis")
}

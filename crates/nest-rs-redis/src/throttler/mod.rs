//! Redis-backed rate-limit store (`throttler` feature) — the cross-process
//! [`ThrottlerStore`](nest_rs_throttler::ThrottlerStore) backend for the
//! `nest-rs-throttler` guard.

mod module;
mod store;

pub use module::RedisThrottlerModule;
#[cfg(test)]
pub(crate) use store::BUCKETS;
pub use store::RedisThrottler;

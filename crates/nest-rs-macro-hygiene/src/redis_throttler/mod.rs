//! The rate limiter's Redis binding, under the umbrella's `redis-throttler`
//! feature alone: `redis` is the connection and nothing more.

mod module;

pub use module::MacroHygieneRedisThrottlerModule;

//! Rate limiting for nestrs.
//!
//! Import [`ThrottlerModule::for_root`] (env-driven, default
//! `Throttle::per_minute(60)`), bind [`ThrottlerGuard`] per route with
//! `#[use_guards(ThrottlerGuard)]`, optionally override per route with
//! `#[meta(Throttle::...)]`. Over-limit requests get `429 Too Many Requests`.
//! Backed by an in-memory fixed-window counter ([`InMemoryThrottler`]).

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — Rate-limit verdicts.
pub const TARGET: &str = "nest_rs::throttler";

mod config;
mod guard;
mod module;
mod pseudonym;
mod store;
mod throttle;

pub use config::ThrottlerConfig;
pub use guard::ThrottlerGuard;
pub use module::{ThrottlerModule, ThrottlerSetup};
pub use store::{BACKEND_REMEDY, Decision, HIT_TIMEOUT, InMemoryThrottler, ThrottlerStore};
pub use throttle::{DEFAULT_THROTTLE, Throttle};

//! Rate limiting for nestrs.
//!
//! Import [`ThrottlerModule::for_root`] (env-driven, default
//! `Throttle::per_minute(60)`), bind [`ThrottlerGuard`] per route with
//! `#[use_guards(ThrottlerGuard)]`, optionally override per route with
//! `#[meta(Throttle::...)]`. Over-limit requests get `429 Too Many Requests`.
//! Backed by an in-memory fixed-window counter ([`InMemoryThrottler`]).

mod config;
mod guard;
mod module;
mod store;
mod throttle;

pub use config::ThrottlerConfig;
pub use guard::ThrottlerGuard;
pub use module::{ThrottlerModule, ThrottlerSetup, DEFAULT_THROTTLE};
pub use store::{Decision, InMemoryThrottler};
pub use throttle::Throttle;

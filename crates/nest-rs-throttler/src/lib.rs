//! Rate limiting for nestrs.
//!
//! Import [`ThrottlerModule::for_root`] (env-driven, default
//! `Throttle::per_minute(60)`), bind [`ThrottlerGuard`] per route with
//! `#[use_guards(ThrottlerGuard)]`, optionally override per route with
//! `#[meta(Throttle::...)]`. Over-limit requests get `429 Too Many Requests`.
//! Backed by an in-memory fixed-window counter ([`InMemoryThrottler`]).

#![warn(missing_docs)]

/// This crate's span target — Rate-limit verdicts.
///
/// Declared by the crate that **owns** the concern, which is not always the only
/// crate emitting on it: a sibling and a `*-macros` expansion read this constant
/// rather than spelling a second one, because a target's one job is to say
/// **where** an event came from. A central table in the kernel would have meant
/// `nest-rs-core` holding a name for a concern it does not know exists.
pub const TARGET: &str = "nest_rs::throttler";

mod config;
mod guard;
mod module;
mod store;
mod throttle;

pub use config::ThrottlerConfig;
pub use guard::ThrottlerGuard;
pub use module::{BACKEND_REMEDY, ThrottlerModule, ThrottlerSetup};
pub use store::{Decision, InMemoryThrottler, ThrottlerStore};
pub use throttle::{DEFAULT_THROTTLE, Throttle};

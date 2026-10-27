//! The Redis connection's indicator, under the umbrella's `redis` and `health`
//! features alone: `health` forwards it weakly, and the connection's own module
//! attaches it.

mod module;

pub use module::MacroHygieneRedisHealthModule;

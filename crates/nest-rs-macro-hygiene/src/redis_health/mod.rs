//! The health port's Redis binding, under the umbrella's `redis` and `health`
//! features alone: `health` forwards it weakly, beside the connection.

mod module;

pub use module::MacroHygieneRedisHealthModule;

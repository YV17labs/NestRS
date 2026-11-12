//! The schedule's Redis binding, under the umbrella's `redis-schedule` feature
//! alone: `redis` is the connection and nothing more.

mod module;

pub use module::MacroHygieneRedisScheduleModule;

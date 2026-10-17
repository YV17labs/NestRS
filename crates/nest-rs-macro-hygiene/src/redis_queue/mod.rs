//! The queue's Redis binding, under the umbrella's `redis-queue` feature alone:
//! `redis` is the connection and nothing more.

mod module;

pub use module::MacroHygieneRedisQueueModule;

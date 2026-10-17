//! The queue port's Redis binding (`queue` feature): [`RedisQueueProducer`]
//! files jobs, a consumer runs them under the port's worker, and
//! [`RedisQueueModule`] binds both over the shared
//! [`RedisConnection`](crate::RedisConnection). Every transition is one of
//! [`scripts`]' Lua scripts, over the keys [`layout`] tabulates.

mod backend;
mod checkpoint;
mod config;
mod consumer;
mod error;
mod layout;
mod module;
mod producer;
mod scripts;

pub(crate) use config::LEASE;
pub use config::RedisQueueConfig;
pub use module::{RedisQueueModule, RedisQueueSetup};
pub use producer::RedisQueueProducer;

//! The queue port's Redis binding: [`RedisQueueProducer`] files jobs, a
//! consumer runs them under the port's worker, and [`RedisQueueModule`] binds
//! both over the shared [`RedisConnection`](crate::RedisConnection). Every
//! transition is one of [`scripts`]' Lua scripts.

mod checkpoint;
mod config;
mod consumer;
mod module;
mod producer;
mod scripts;

pub(crate) use config::LEASE;
pub use config::RedisQueueConfig;
pub use module::{RedisQueueModule, RedisQueueSetup};
pub use producer::RedisQueueProducer;

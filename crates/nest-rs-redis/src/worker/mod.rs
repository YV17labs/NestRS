//! Consumer side: the [`RedisWorker`] transport drains `#[process]` methods
//! discovered at link time, and [`RedisWorkerModule`] is the activation seam
//! a worker app imports to attach the transport. Producer-only apps skip
//! this module — [`crate::RedisQueueModule`] is the producer side, and both
//! read the connection [`crate::RedisModule`] opens.

mod checkpoint;
mod config;
mod consumer;
mod delivery;
mod gate;
mod lease;
mod module;

pub use config::RedisWorkerConfig;
pub use consumer::RedisWorker;
pub use module::{RedisWorkerModule, RedisWorkerSetup};

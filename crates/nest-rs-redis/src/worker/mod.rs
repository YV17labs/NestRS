//! Consumer side: the [`RedisWorker`] transport drains `#[process]` methods
//! discovered at link time, and [`RedisWorkerModule`] is the activation seam
//! a worker app imports to attach the transport. Producer-only apps skip
//! this module — see [`crate::RedisQueueModule`] for the connection side.

mod consumer;
mod module;

pub use consumer::RedisWorker;
pub use module::RedisWorkerModule;

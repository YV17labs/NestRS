//! Consumer side: the [`QueueWorker`] transport drains `#[process]` methods
//! discovered at link time, and [`QueueWorkerModule`] is the activation seam
//! a worker app imports to attach the transport. Producer-only apps skip
//! this module — see [`crate::QueueModule`] for the connection side.

mod consumer;
mod module;

pub use consumer::QueueWorker;
pub use module::QueueWorkerModule;

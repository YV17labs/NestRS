//! The queue port's producer half: [`RedisQueueProducer`] over the shared
//! [`RedisConnection`](crate::RedisConnection), and the [`RedisQueueModule`]
//! binding that registers it.
//!
//! One folder per port bound, like [`throttler`](crate::throttler) and
//! [`worker`](crate::worker) beside it; what the three share — the connection
//! and its config — sits at the crate root.

mod module;
mod producer;

pub use module::RedisQueueModule;
pub use producer::RedisQueueProducer;

//! The queue backend kit (feature `queue`): the behaviour every queue backend
//! owes the port's worker, as tests a driver runs against its own
//! implementation with [`queue_kit!`](crate::queue_kit) — a delivery, its
//! concurrency, its retry and its dead letter, a worker dying between the fetch
//! and the settle, a lease lost under a live worker, the stall limit, the
//! drain, the lease renewed through a long attempt, a delayed push and the
//! trace a job carries. `nest-rs-queue` runs it against an in-memory backend
//! and `nest-rs-redis` against Redis.

mod kit;

pub use kit::KitBackend;
#[doc(hidden)]
pub use kit::{cases, run};

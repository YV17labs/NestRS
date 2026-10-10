//! The queue backend kit (feature `queue`): the behaviour every queue backend
//! owes the port's worker, as tests a driver runs against its own
//! implementation with [`queue_kit!`](crate::queue_kit).

pub(crate) mod kit;

pub use kit::KitBackend;

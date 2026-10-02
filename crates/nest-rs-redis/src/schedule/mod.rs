//! The schedule port's occurrence lock (`schedule` feature):
//! [`RedisOccurrenceLock`] over the shared connection, and the
//! [`RedisScheduleModule`] binding that declares it.

mod lock;
mod module;

#[cfg(test)]
pub(crate) use lock::CLAIMS;
pub use lock::RedisOccurrenceLock;
pub use module::RedisScheduleModule;

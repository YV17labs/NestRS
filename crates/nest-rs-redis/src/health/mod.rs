//! The health port's indicator (`health` feature): [`RedisHealthModule`]
//! attaches each instance's `PING`, named for the crate and the instance.

mod indicator;
mod module;

pub use module::RedisHealthModule;

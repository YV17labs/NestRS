//! Each instance's indicator (`health` feature): a `PING`, named for the crate
//! and the instance, which [`RedisModule::for_root`](crate::RedisModule::for_root)
//! attaches for the instance it opens.

mod indicator;

pub(crate) use indicator::register;

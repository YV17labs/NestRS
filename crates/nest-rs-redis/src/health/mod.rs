//! Each instance's indicator (`health` feature): a `PING`, named for the crate
//! and the instance, which [`RedisModule`](crate::RedisModule) attaches for the
//! instance it opens.

mod indicator;

pub(crate) use indicator::register;

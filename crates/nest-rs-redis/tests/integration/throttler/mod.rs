//! The store `RedisThrottlerModule` binds, in process: [`composition`] for what
//! the boot refuses before Redis is dialled, [`store`] for a Redis that stops
//! answering under the guard.

mod composition;
mod store;

//! [`RedisScheduleModule`] — the Redis binding of the schedule port's occurrence
//! lock. A bare import beside `ScheduleModule` and
//! [`RedisModule::for_root`](crate::RedisModule::for_root): it declares
//! [`RedisOccurrenceLock`] as the `dyn OccurrenceLock` a job declared
//! `replicas = "one"` claims its occurrences through. Enabled by the `schedule`
//! feature.

use std::sync::Arc;

use nest_rs_core::{Collecting, ContainerBuilder, Module, Registering};
use nest_rs_schedule::OccurrenceLock;

use crate::RedisConnection;
use crate::schedule::RedisOccurrenceLock;

/// Claims every occurrence of a `replicas = "one"` job in Redis. Import beside
/// `ScheduleModule` in each app that runs such a job; its replicas then share
/// one set of claims, and each occurrence fires on at most one of them.
pub struct RedisScheduleModule;

impl Module for RedisScheduleModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        // Declared, so a second lock backend contests it by name
        // (`BACKEND_REMEDY`); queued after the connection's factory, so
        // `imports` order is not a wiring mistake a reader has to know about.
        RedisConnection::netted(builder, "the scheduler", nest_rs_schedule::LOCK_TIMEOUT)
            .provide_declared_factory_after::<Arc<dyn OccurrenceLock>, RedisConnection, _, _>(
            nest_rs_schedule::BACKEND_REMEDY,
            |container| async move {
                let conn = RedisConnection::of(&container, "RedisScheduleModule")?;
                Ok(Arc::new(RedisOccurrenceLock::new(conn)) as Arc<dyn OccurrenceLock>)
            },
        )
    }
}

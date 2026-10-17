//! [`RedisScheduleModule`] — the Redis binding of the schedule port's occurrence
//! lock. A bare import beside `ScheduleModule` and
//! [`RedisModule::for_root`](crate::RedisModule::for_root): it declares
//! [`RedisOccurrenceLock`] as the `dyn OccurrenceLock` a job declared
//! `replicas = "one"` claims its occurrences through. Enabled by the `schedule`
//! feature.

use std::marker::PhantomData;
use std::sync::Arc;

use nest_rs_core::{Collecting, ContainerBuilder, Instance, Module, Registering, Unnamed};
use nest_rs_schedule::OccurrenceLock;

use crate::RedisConnection;
use crate::schedule::RedisOccurrenceLock;

/// Claims every occurrence of a `replicas = "one"` job in Redis. Import beside
/// `ScheduleModule` in each app that runs such a job; its replicas then share
/// one set of claims, and each occurrence fires on at most one of them. It
/// claims on the default instance's connection, or on a named one's when
/// imported as `RedisScheduleModule::<Locks>`.
pub struct RedisScheduleModule<I = Unnamed>(PhantomData<fn() -> I>);

impl<I: Instance> Module for RedisScheduleModule<I> {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        // Declared, so a second lock backend contests it by name (`BACKEND_REMEDY`).
        RedisConnection::<I>::netted(builder, "the scheduler", nest_rs_schedule::LOCK_TIMEOUT)
            .provide_declared_factory_after::<Arc<dyn OccurrenceLock>, (RedisConnection<I>,), _, _>(
                nest_rs_schedule::BACKEND_REMEDY,
                |container| async move {
                    let conn = RedisConnection::<I>::of(&container, "RedisScheduleModule")?;
                    Ok(Arc::new(RedisOccurrenceLock::new(conn)) as Arc<dyn OccurrenceLock>)
                },
            )
    }
}

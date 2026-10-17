//! [`RedisQueueModule`] — binds the queue port over the shared
//! [`RedisConnection`]: the producer a feature pushes through, and the consumer
//! the port's `QueueWorker` runs jobs with when the app imports
//! `QueueModule`.

use std::borrow::Borrow;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_config::{ConfigModule, ConfigSetup, ForInstance, Namespaced, instance_namespace};
use nest_rs_core::{Collecting, ContainerBuilder, Instance, Module, Registering, Unnamed};
use nest_rs_queue::{BACKEND_REMEDY, BACKEND_TIMEOUT, BoundConsumer, JobProducer};

use super::consumer::RedisQueueConsumer;
use super::{LEASE, RedisQueueConfig, RedisQueueProducer};
use crate::instance::RedisInstance;
use crate::{RedisConnection, RedisError};

/// The Redis queue binding. Import it beside
/// [`RedisModule::for_root`](crate::RedisModule::for_root): an app pushing jobs
/// injects `Arc<dyn JobProducer>`, and an app running them imports
/// `QueueModule` beside it too. A bare import reads `<PREFIX>_REDIS__QUEUE__*`;
/// [`for_root`](RedisQueueModule::for_root) pins the base those variables
/// overlay. It queues on the default instance's connection, or on a named
/// one's — `RedisQueueModule::<Jobs>` reading `<PREFIX>_REDIS__JOBS__QUEUE__*`.
pub struct RedisQueueModule<I = Unnamed>(PhantomData<fn() -> I>);

impl RedisQueueModule {
    /// `None` loads [`RedisQueueConfig`] from `<PREFIX>_REDIS__QUEUE__*`;
    /// `Some(cfg)` pins the base those variables overlay, field by field. A
    /// queue on a named instance takes `(Marker, x)` and reads
    /// `<PREFIX>_REDIS__<INSTANCE>__QUEUE__*`.
    pub fn for_root<I: Instance>(
        config: impl Into<ForInstance<I, RedisQueueConfig>>,
    ) -> RedisQueueSetup<I> {
        ConfigModule::setup(config)
    }
}

impl<I: Instance> RedisQueueModule<I> {
    /// The producer and the consumer, each bound under the port's type and
    /// declared, so a second queue backend imported beside this one fails the
    /// boot naming both ([`BACKEND_REMEDY`]); and the port's net over the
    /// connection, which the boot holds the connection's budget under.
    fn bind(builder: ContainerBuilder) -> ContainerBuilder {
        RedisConnection::<I>::netted(builder, "the queue port", BACKEND_TIMEOUT)
            .provide_factory_after::<RedisQueueProducer<I>, (RedisConnection<I>,), _, _>(
                |container| async move {
                    let producer = RedisQueueProducer::new(RedisConnection::<I>::of(
                        &container,
                        "RedisQueueModule",
                    )?);
                    producer.load_scripts().await?;
                    Ok(producer)
                },
            )
            .provide_declared_factory_after::<Arc<dyn JobProducer>, (RedisQueueProducer<I>,), _, _>(
                BACKEND_REMEDY,
                |container| async move {
                    let producer = container.get::<RedisQueueProducer<I>>().ok_or_else(|| {
                        anyhow::anyhow!("RedisQueueModule: RedisQueueProducer was not resolved")
                    })?;
                    Ok(producer as Arc<dyn JobProducer>)
                },
            )
            .provide_declared_factory_after::<
                BoundConsumer,
                (RedisConnection<I>, I::Of<RedisQueueConfig>),
                _,
                _,
            >(BACKEND_REMEDY, |container| async move {
                let conn = RedisConnection::<I>::of(&container, "RedisQueueModule")?;
                let held = ConfigModule::resolved_for::<I, RedisQueueConfig>(&container)?;
                let config: &RedisQueueConfig = (*held).borrow();
                renewable::<I>(config.lease, conn.budget())?;
                Ok(BoundConsumer::new(RedisQueueConsumer::new(conn, config.lease)))
            })
    }
}

impl<I: Instance> Module for RedisQueueModule<I> {
    // A bare import still reads `<PREFIX>_REDIS__QUEUE__*`; a
    // `for_root(Some(cfg))`'s declared value supersedes this env-only one.
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        Self::bind(ConfigModule::provide_feature(
            ForInstance::<I, RedisQueueConfig>::default(),
            builder,
        ))
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

/// The configured import [`RedisQueueModule::for_root`] returns: the config it
/// pins, and the bindings — queued once, however many sites import the module.
pub type RedisQueueSetup<I = Unnamed> = ConfigSetup<RedisQueueModule<I>, RedisQueueConfig, I>;

/// `Ok` when the port's renewal fits in `lease` though it waits out the whole
/// `budget`, else the refusal naming the instance `I`'s variables.
fn renewable<I: Instance>(lease: Duration, budget: Duration) -> Result<(), RedisError> {
    if nest_rs_queue::lease_fits_renewal(lease, budget) {
        Ok(())
    } else {
        Err(RedisError::BudgetPastLease {
            budget,
            lease,
            lease_var: nest_rs_config::var_name(
                &instance_namespace::<I>(RedisQueueConfig::NAMESPACE),
                LEASE.key(),
            ),
            connect_timeout_var: RedisInstance::of::<I>().connect_timeout_var(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_fit_and_a_lease_no_renewal_fits_is_refused() {
        let budget = crate::RedisConfig::default().connect_timeout;
        assert!(renewable::<Unnamed>(RedisQueueConfig::default().lease, budget).is_ok());
        let Err(refused) = renewable::<Unnamed>(Duration::from_secs(1), Duration::from_secs(1))
        else {
            panic!("a lease no longer than the budget must be refused");
        };
        assert!(
            matches!(refused, RedisError::BudgetPastLease { .. }),
            "{refused}"
        );
        let rendered = refused.to_string();
        assert!(
            rendered.contains(&nest_rs_config::var_name("redis__queue", "LEASE_SECS"))
                && rendered.contains(&nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS")),
            "{rendered}"
        );
    }

    #[nest_rs_core::instance]
    struct Jobs;

    #[test]
    fn a_lease_refused_on_a_named_instance_names_that_instance_s_variables() {
        let Err(refused) = renewable::<Jobs>(Duration::from_secs(1), Duration::from_secs(1)) else {
            panic!("a lease no longer than the budget must be refused");
        };
        let rendered = refused.to_string();
        assert!(
            rendered.contains(&nest_rs_config::var_name(
                "redis__jobs__queue",
                "LEASE_SECS"
            )) && rendered.contains(&nest_rs_config::var_name(
                "redis__jobs",
                "CONNECT_TIMEOUT_SECS"
            )),
            "{rendered}"
        );
    }
}

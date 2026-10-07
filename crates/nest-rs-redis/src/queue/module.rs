//! [`RedisQueueModule`] — binds the queue port over the shared
//! [`RedisConnection`]: the producer a feature pushes through, and the consumer
//! the port's `QueueWorker` runs jobs with when the app imports
//! `QueueModule`.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::{Collecting, ContainerBuilder, Module, Registering};
use nest_rs_queue::{BACKEND_REMEDY, BACKEND_TIMEOUT, BoundConsumer, JobProducer};

use super::consumer::RedisQueueConsumer;
use super::{RedisQueueConfig, RedisQueueProducer};
use crate::{RedisConnection, RedisError};

/// The Redis queue binding. Import it beside
/// [`RedisModule::for_root`](crate::RedisModule::for_root): an app pushing jobs
/// injects `Arc<dyn JobProducer>`, and an app running them imports
/// `QueueModule` beside it too. A bare import reads `<PREFIX>_REDIS__QUEUE__*`;
/// [`for_root`](Self::for_root) pins the base those variables overlay.
pub struct RedisQueueModule;

impl RedisQueueModule {
    /// `None` loads [`RedisQueueConfig`] from `<PREFIX>_REDIS__QUEUE__*`;
    /// `Some(cfg)` pins the base those variables overlay, field by field.
    pub fn for_root(config: impl Into<Option<RedisQueueConfig>>) -> RedisQueueSetup {
        ConfigModule::setup(config)
    }

    /// The producer and the consumer, each bound under the port's type and
    /// declared, so a second queue backend imported beside this one fails the
    /// boot naming both ([`BACKEND_REMEDY`]); and the port's net over the
    /// connection, which the boot holds the connection's budget under.
    fn bind(builder: ContainerBuilder) -> ContainerBuilder {
        RedisConnection::netted(builder, "the queue port", BACKEND_TIMEOUT)
            .provide_factory_after::<RedisQueueProducer, RedisConnection, _, _>(
                |container| async move {
                    let producer =
                        RedisQueueProducer::new(RedisConnection::of(&container, "RedisQueueModule")?);
                    producer.load_scripts().await?;
                    Ok(producer)
                },
            )
            .provide_declared_factory_after::<Arc<dyn JobProducer>, RedisQueueProducer, _, _>(
                BACKEND_REMEDY,
                |container| async move {
                    let producer = container.get::<RedisQueueProducer>().ok_or_else(|| {
                        anyhow::anyhow!("RedisQueueModule: RedisQueueProducer was not resolved")
                    })?;
                    Ok(producer as Arc<dyn JobProducer>)
                },
            )
            .provide_declared_factory_after_both::<BoundConsumer, RedisConnection, RedisQueueConfig, _, _>(
                BACKEND_REMEDY,
                |container| async move {
                    let conn = RedisConnection::of(&container, "RedisQueueModule")?;
                    let config = container.get::<RedisQueueConfig>().ok_or_else(|| {
                        anyhow::anyhow!("RedisQueueModule: RedisQueueConfig was not resolved")
                    })?;
                    renewable(config.lease, conn.budget())?;
                    Ok(BoundConsumer::new(RedisQueueConsumer::new(conn, config.lease)))
                },
            )
    }
}

impl Module for RedisQueueModule {
    // A bare import still reads `<PREFIX>_REDIS__QUEUE__*`; a
    // `for_root(Some(cfg))`'s declared value supersedes this env-only one.
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        Self::bind(ConfigModule::provide_feature(
            None::<RedisQueueConfig>,
            builder,
        ))
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

/// The configured import [`RedisQueueModule::for_root`] returns: the config it
/// pins, and the bindings — queued once, however many sites import the module.
pub type RedisQueueSetup = ConfigSetup<RedisQueueModule, RedisQueueConfig>;

/// `Ok` when the port's renewal fits in `lease` though it waits out the whole
/// `budget`.
fn renewable(lease: Duration, budget: Duration) -> Result<(), RedisError> {
    if nest_rs_queue::lease_fits_renewal(lease, budget) {
        Ok(())
    } else {
        Err(RedisError::BudgetPastLease { budget, lease })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defaults fit, and a lease the port's renewal does not fit is refused
    /// naming both settings.
    #[test]
    fn the_defaults_fit_and_a_lease_no_renewal_fits_is_refused() {
        let budget = crate::RedisConfig::default().connect_timeout;
        assert!(renewable(RedisQueueConfig::default().lease, budget).is_ok());
        let Err(refused) = renewable(Duration::from_secs(1), Duration::from_secs(1)) else {
            panic!("a lease no longer than the budget must be refused");
        };
        assert!(
            matches!(refused, RedisError::BudgetPastLease { .. }),
            "{refused}"
        );
    }
}

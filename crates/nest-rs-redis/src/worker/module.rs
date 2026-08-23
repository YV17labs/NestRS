//! The activation seam for the consumer side: import [`RedisWorkerModule`]
//! in a worker app's `#[module(imports = [...])]` and the framework
//! attaches the [`RedisWorker`] to the app at boot.
//!
//! Separated from [`RedisQueueModule`](crate::RedisQueueModule) so a producer-only app
//! (the API push side) can import the producer binding without draining the
//! processor inventory and spawning a consumer it does not need. Both read the
//! connection [`RedisModule::for_root`](crate::RedisModule::for_root) opens.

use std::any::TypeId;

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::{ContainerBuilder, Module, TransportContribution};

use super::{RedisWorker, RedisWorkerConfig};

/// The consumer-side activation seam. Import `RedisWorkerModule::for_root(None)`
/// in a worker app to attach the [`RedisWorker`](crate::RedisWorker) transport;
/// a producer-only app omits it.
pub struct RedisWorkerModule;

impl RedisWorkerModule {
    /// `None` ⇒ load [`RedisWorkerConfig`] from `NESTRS_REDIS__WORKER__*`;
    /// `Some(cfg)` pins the base those variables overlay, per field.
    pub fn for_root(config: impl Into<Option<RedisWorkerConfig>>) -> RedisWorkerSetup {
        ConfigModule::setup(config)
    }
}

/// The configured import produced by [`RedisWorkerModule::for_root`]: the
/// shared `ConfigSetup`, since the seam only pins the config and then recurses
/// into the module's own wiring.
pub type RedisWorkerSetup = ConfigSetup<RedisWorkerModule, RedisWorkerConfig>;

impl Module for RedisWorkerModule {
    // A bare import still loads `NESTRS_REDIS__WORKER__*`: the env-only factory
    // is queued here, and a `for_root(Some(cfg))`'s declared one supersedes it.
    // Without this the bare form booted, served, and dropped the operator's
    // drain window in silence.
    fn collect(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_collected(TypeId::of::<Self>()) {
            return builder;
        }
        ConfigModule::provide_feature(None::<RedisWorkerConfig>, builder)
    }

    // Deduped like a `#[module]` expansion: two importers attach one transport,
    // not two consumers each running one job at a time per method.
    fn register(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_registered(TypeId::of::<Self>()) {
            return builder;
        }
        builder.provide_meta(TransportContribution {
            name: "RedisWorker",
            build: |_| Ok(Box::new(RedisWorker::new())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_core::{Container, DiscoveryService};

    #[test]
    fn registering_the_module_attaches_one_transport_contribution() {
        let container = RedisWorkerModule::register(Container::builder()).build();
        let contributions = DiscoveryService::new(&container).meta::<TransportContribution>();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].meta.name, "RedisWorker");
    }
}

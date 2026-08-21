//! The activation seam for the consumer side: import [`RedisWorkerModule`]
//! in a worker app's `#[module(imports = [...])]` and the framework
//! attaches the [`RedisWorker`] to the app at boot.
//!
//! Separated from [`RedisQueueModule`](crate::RedisQueueModule) so a producer-only app
//! (the API push side) can import [`RedisQueueModule::for_root(...)`](crate::RedisQueueModule::for_root)
//! to gain [`RedisQueueConnection`](crate::RedisQueueConnection) without draining the
//! processor inventory and spawning a consumer it does not need.

use nest_rs_core::{ContainerBuilder, Module, TransportContribution};

use super::RedisWorker;

/// The consumer-side activation seam. Import it in a worker app to attach the
/// [`RedisWorker`](crate::RedisWorker) transport; a producer-only app omits it.
pub struct RedisWorkerModule;

impl Module for RedisWorkerModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
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

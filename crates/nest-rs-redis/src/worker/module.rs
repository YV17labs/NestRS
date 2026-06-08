//! The activation seam for the consumer side: import [`QueueWorkerModule`]
//! in a worker app's `#[module(imports = [...])]` and the framework
//! attaches the [`QueueWorker`] to the app at boot.
//!
//! Separated from [`QueueModule`](crate::QueueModule) so a producer-only app
//! (the API push side) can import [`QueueModule::for_root(...)`](crate::QueueModule::for_root)
//! to gain [`QueueConnection`](crate::QueueConnection) without draining the
//! processor inventory and spawning a consumer it does not need.

use nest_rs_core::{ContainerBuilder, Module, TransportContribution};

use super::QueueWorker;

pub struct QueueWorkerModule;

impl Module for QueueWorkerModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide_meta(TransportContribution {
            name: "QueueWorker",
            build: |_| Ok(Box::new(QueueWorker::new())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_core::{Container, DiscoveryService};

    #[test]
    fn registering_the_module_attaches_one_transport_contribution() {
        let container = QueueWorkerModule::register(Container::builder()).build();
        let contributions = DiscoveryService::new(&container).meta::<TransportContribution>();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].meta.name, "QueueWorker");
    }
}

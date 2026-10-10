//! [`QueueModule`] — the activation seam of the consumer side: imported, it
//! attaches the [`QueueWorker`] that runs every reachable
//! `#[process]` method over the queue backend a binding bound.

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::__private::TransportContribution;
use nest_rs_core::{Collecting, ContainerBuilder, Module, Registering};

use crate::{QueueConfig, QueueWorker};

/// Attaches the [`QueueWorker`] to a worker app. A
/// producer-only app omits it; a backend binding beside it (Redis's
/// `RedisQueueModule`) supplies the consumer the worker runs.
pub struct QueueModule;

impl QueueModule {
    /// `None` loads [`QueueConfig`] from `<PREFIX>_QUEUE__*`; `Some(cfg)` pins
    /// the base those variables overlay, field by field.
    pub fn for_root(config: impl Into<Option<QueueConfig>>) -> QueueSetup {
        ConfigModule::setup(config)
    }
}

/// The configured import [`QueueModule::for_root`] returns.
pub type QueueSetup = ConfigSetup<QueueModule, QueueConfig>;

impl Module for QueueModule {
    // A bare import still reads `<PREFIX>_QUEUE__*`; a `for_root(Some(cfg))`'s
    // declared value supersedes this env-only one.
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature(None::<QueueConfig>, builder)
    }

    // Registered once however many modules import it, so two importers attach
    // one worker, never two pools of permits running each method twice over.
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_meta(TransportContribution {
            name: "QueueWorker",
            build: |_| Ok(Box::new(QueueWorker::new())),
        })
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{App, Container, Discovery, LateFactoryError};

    use super::*;

    /// A hand-written importer that imports the module in its register alone.
    struct RegistersOnly;

    impl Module for RegistersOnly {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.import::<QueueModule>()
        }
    }

    #[tokio::test]
    async fn the_module_imported_in_a_register_alone_fails_the_boot_naming_its_config() {
        let Err(refused) = App::builder().module::<RegistersOnly>().build().await else {
            panic!("the worker would run on a config nothing resolved");
        };
        let late = refused
            .downcast_ref::<LateFactoryError>()
            .unwrap_or_else(|| panic!("not the late-factory refusal: {refused:#}"));
        assert_eq!(late.type_name, std::any::type_name::<QueueConfig>());
    }

    #[test]
    fn two_imports_attach_one_worker() {
        let container = Container::builder()
            .import::<QueueModule>()
            .import::<QueueModule>()
            .build();
        let contributions = Discovery::new(&container).meta::<TransportContribution>();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].meta.name, "QueueWorker");
    }
}

//! [`QueueModule`] — the activation seam of the consumer side: imported, it
//! attaches the [`QueueWorker`] that runs every reachable
//! `#[process]` method over the queue backend a binding bound.

use std::any::TypeId;

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::{ContainerBuilder, Module, TransportContribution};

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
    fn collect(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_collected(TypeId::of::<Self>()) {
            return builder;
        }
        ConfigModule::provide_feature(None::<QueueConfig>, builder)
    }

    // Deduplicated: two importers attach one worker, never two pools of
    // permits running each method twice over.
    fn register(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_registered(TypeId::of::<Self>()) {
            return builder;
        }
        // A no-op once collected; an importer that skipped `collect` gets what
        // it queues refused by name (`LateFactoryError`), never left unbuilt.
        Self::collect(builder).provide_meta(TransportContribution {
            name: "QueueWorker",
            build: |_| Ok(Box::new(QueueWorker::new())),
        })
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{App, Container, Discovery, LateFactoryError};

    use super::*;

    /// A hand-written importer that registers the module without collecting it.
    struct RegistersOnly;

    impl Module for RegistersOnly {
        fn register(builder: ContainerBuilder) -> ContainerBuilder {
            QueueModule::register(builder)
        }
    }

    #[tokio::test]
    async fn the_module_registered_without_its_collect_fails_the_boot_naming_its_config() {
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
        let builder = QueueModule::register(Container::builder());
        let container = QueueModule::register(builder).build();
        let contributions = Discovery::new(&container).meta::<TransportContribution>();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].meta.name, "QueueWorker");
    }
}

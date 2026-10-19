//! [`OutboxModule`] — attaches the [`OutboxRelay`], which files what units
//! wrote into the outbox a data driver keeps.

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::{Collecting, ContainerBuilder, Module, Registering};

use super::{OutboxConfig, OutboxRelay};

/// Attaches the [`OutboxRelay`]. A data driver's binding imports it bare beside
/// the `Arc<dyn OutboxStore>` it binds — SeaORM's
/// `SeaOrmDatabaseModule` does, under the umbrella's `queue` feature — so an app
/// binding a database and a queue backend lists nothing; the relay idles where
/// either is missing. An app pins its settings with
/// [`for_root`](OutboxModule::for_root).
pub struct OutboxModule;

impl OutboxModule {
    /// `None` loads [`OutboxConfig`] from `<PREFIX>_QUEUE__OUTBOX__*`;
    /// `Some(cfg)` pins the base those variables overlay, field by field.
    pub fn for_root(config: impl Into<Option<OutboxConfig>>) -> OutboxSetup {
        ConfigModule::setup(config.into())
    }
}

/// The configured import [`OutboxModule::for_root`] returns.
pub type OutboxSetup = ConfigSetup<OutboxModule, OutboxConfig>;

impl Module for OutboxModule {
    // A bare import still reads `<PREFIX>_QUEUE__OUTBOX__*`; a
    // `for_root(Some(cfg))`'s declared value supersedes this env-only one.
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature(None::<OutboxConfig>, builder)
    }

    // Registered once however many bindings import it: one relay per app.
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transport(|_| Ok(OutboxRelay::new()))
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{Container, Discovery, TransportContribution};

    use super::*;

    #[test]
    fn two_imports_attach_one_relay() {
        let container = Container::builder()
            .provide(OutboxConfig::default())
            .import::<OutboxModule>()
            .import::<OutboxModule>()
            .build()
            .expect("the container builds");
        let contributions = Discovery::new(&container).meta::<TransportContribution>();
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].meta.name(), "OutboxRelay");
    }
}

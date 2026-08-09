//! Wires the shared [`Storage`] provider and its [`StorageConfig`].
//!
//! `Storage` is built lazily on first use (see [`Storage`]), so the module only
//! has to register the provider and feed it the config loaded from
//! `NESTRS_STORAGE__*`.
//!
//! Importing the bare [`StorageModule`] declares the dependency and leaves the
//! config to the environment; [`StorageModule::for_root`] supplies a base those
//! variables overlay, so a bucket pinned in code is still overridable per field
//! by the deployment (see `nest_rs_config::Config`).

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::module;

use crate::client::Storage;
use crate::config::StorageConfig;

/// Provides the S3 [`Storage`](crate::Storage) client from [`StorageConfig`].
/// Import it to inject `Storage` for presigned URLs, uploads and metadata reads.
#[module(
    imports = [ConfigModule::for_feature::<StorageConfig>()],
    providers = [Storage],
)]
pub struct StorageModule;

impl StorageModule {
    /// `None` ⇒ load [`StorageConfig`] from `NESTRS_STORAGE__*` over its
    /// defaults; `Some(cfg)` makes `cfg` the base those variables overlay.
    /// Either way [`Storage`] is provided, so this is a drop-in replacement for
    /// importing the bare [`StorageModule`].
    pub fn for_root(config: impl Into<Option<StorageConfig>>) -> StorageSetup {
        ConfigModule::setup(config)
    }
}

/// [`DynamicModule`] returned by [`StorageModule::for_root`]: resolves
/// [`StorageConfig`] (env over the pinned base), then brings the base
/// [`StorageModule`] wiring. The factory is queued first, so it wins over — and
/// skips — the plain env factory the base module queues.
///
/// Pin-and-recurse is the whole behaviour, so it is [`ConfigSetup`] rather than
/// a type of its own.
pub type StorageSetup = ConfigSetup<StorageModule, StorageConfig>;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_core::App;

    use super::*;

    /// Pinned bucket for the test below, as a real import site.
    fn pinned_storage() -> StorageSetup {
        StorageModule::for_root(StorageConfig {
            bucket: "pinned-bucket".into(),
            region: "eu-west-3".into(),
            ..StorageConfig::default()
        })
    }

    #[module(imports = [pinned_storage()])]
    struct PinnedStorageHost;

    /// The seam this crate went without: a `#[config]` reachable only from the
    /// environment breaks the dual-path rule from the other side, and an app had
    /// no way to pin a bucket short of seeding the value — which would have
    /// frozen every other `NESTRS_STORAGE__*` field against the deployment.
    #[tokio::test]
    async fn for_root_pins_the_config_and_still_provides_the_client() {
        let app = App::builder()
            .module::<PinnedStorageHost>()
            .build()
            .await
            .expect("the pinned-config module boots");

        let cfg: Option<Arc<StorageConfig>> = app.container().get();
        assert_eq!(
            cfg.expect("pinned StorageConfig resolves").bucket,
            "pinned-bucket",
        );
        let storage: Option<Arc<Storage>> = app.container().get();
        assert!(storage.is_some(), "for_root still provides the client");
    }
}

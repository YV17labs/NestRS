//! Wires the shared [`Storage`] provider, the store it opens at boot, and its
//! [`StorageConfig`].

use std::any::TypeId;

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule, module};

use crate::client::Storage;
use crate::config::StorageConfig;
use crate::store::Store;

/// Provides the [`Storage`](crate::Storage) client over the store
/// [`StorageConfig::url`] names, opened and proved at boot. Import it to inject
/// `Storage` for presigned URLs, uploads and metadata reads.
#[module(
    imports = [ConfigModule::for_feature::<StorageConfig>(), StorageOpening::import()],
    providers = [Storage],
)]
pub struct StorageModule;

impl StorageModule {
    /// `None` ⇒ load [`StorageConfig`] from `<PREFIX>_STORAGE__*` over its
    /// defaults; `Some(cfg)` makes `cfg` the base those variables overlay.
    pub fn for_root(config: impl Into<Option<StorageConfig>>) -> StorageSetup {
        ConfigModule::setup(config.into())
    }
}

/// [`DynamicModule`] returned by
/// [`StorageModule::for_root`]: resolves
/// [`StorageConfig`] (env over the pinned base), then brings the base
/// [`StorageModule`] wiring.
pub type StorageSetup = ConfigSetup<StorageModule, StorageConfig>;

/// The import of [`StorageModule`]'s that opens the store in the factory
/// phase, once the config resolved — so the boot proves it before any
/// provider is built; a test overriding `Storage` opens none. Its output is
/// this crate's alone: `Storage`, which a module lists, is what the access
/// graph gates.
struct StorageOpening;

impl StorageOpening {
    /// The import, as `#[module]` takes a configured one.
    fn import() -> Self {
        Self
    }
}

impl DynamicModule for StorageOpening {
    fn module() -> TypeId {
        TypeId::of::<Self>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        if nest_rs_core::__private::overrides_any(&builder, &[TypeId::of::<Storage>()]) {
            return builder;
        }
        builder.provide_factory_after::<Store, (StorageConfig,), _, _>(|container| async move {
            let config = ConfigModule::resolved::<StorageConfig>(&container)?;
            Ok(Store::open(&config).await?)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_authn::{AUTHENTICATE_TIMEOUT, AuthError, AuthnGuard, Strategy};
    use nest_rs_core::{App, BudgetPastNetError, injectable};
    use nest_rs_http::Request;
    use nest_rs_http::async_trait;

    use super::*;

    /// A strategy that injects the client, as one reading a key it keeps in a
    /// bucket does.
    #[injectable]
    struct StorageStrategy {
        #[inject]
        #[expect(
            dead_code,
            reason = "injected only so the strategy's code reaches the client"
        )]
        storage: Arc<Storage>,
    }

    /// The principal the strategy would attach: a caller with no identity.
    #[derive(Clone)]
    struct Anyone;

    impl nest_rs_core::AmbientValue for Anyone {}

    impl nest_rs_authn::PrincipalIdentity for Anyone {
        fn actor_id(&self) -> Option<String> {
            None
        }
    }

    #[async_trait]
    impl Strategy for StorageStrategy {
        type Principal = Anyone;

        async fn authenticate(&self, _req: &mut Request) -> Result<Anyone, AuthError> {
            Err(AuthError::MissingCredentials)
        }
    }

    #[module(
        imports = [StorageModule],
        providers = [StorageStrategy, AuthnGuard<StorageStrategy>],
    )]
    struct GuardedStorageModule;

    /// The in-process store, which a test profile opens with no server.
    fn in_memory() -> StorageConfig {
        nest_rs_testing::load_project_env();
        StorageConfig {
            url: "memory:///".into(),
            ..StorageConfig::default()
        }
    }

    #[tokio::test]
    async fn an_operation_budget_at_the_net_of_a_guard_injecting_the_client_fails_the_boot() {
        let Err(refused) = App::builder()
            .provide(StorageConfig {
                operation_timeout: AUTHENTICATE_TIMEOUT,
                ..in_memory()
            })
            .module::<GuardedStorageModule>()
            .build()
            .await
        else {
            panic!("a client waiting as long as the guard must not boot");
        };
        let refused = refused
            .downcast::<BudgetPastNetError>()
            .unwrap_or_else(|other| panic!("not a budget refusal: {other:#}"));
        assert_eq!(
            (refused.resource, refused.port, refused.budget),
            (
                "the object store",
                "the authentication guard",
                AUTHENTICATE_TIMEOUT
            )
        );
        assert!(
            refused.setting.contains(&nest_rs_config::var_name(
                "storage",
                "OPERATION_TIMEOUT_SECS"
            )),
            "{refused}"
        );
    }

    #[tokio::test]
    async fn the_default_client_boots_beside_the_authentication_guard() {
        App::builder()
            .provide(in_memory())
            .module::<GuardedStorageModule>()
            .build()
            .await
            .expect("the default budget sits under every net");
    }

    /// A store pinned for the test below, as a real import site.
    fn pinned_storage() -> StorageSetup {
        StorageModule::for_root(StorageConfig {
            url: "memory:///pinned".into(),
            ..in_memory()
        })
    }

    #[module(imports = [pinned_storage()])]
    struct PinnedStorageHost;

    /// A client certificate pinned in code, which object_store cannot present.
    fn presenting_storage() -> StorageSetup {
        let issued = nest_rs_testing::TestAuthority::new().client("nestrs-test-client");
        let inline = |pem: String| nest_rs_config::Material {
            bytes: pem.into_bytes(),
            path: None,
        };
        StorageModule::for_root(StorageConfig {
            url: "s3://bucket".into(),
            access_key: "AKIAPINNED".into(),
            secret_key: "pinned-secret".into(),
            tls: nest_rs_config::ClientTls::new(
                None,
                Some(nest_rs_config::TlsIdentity::new(
                    inline(issued.cert),
                    inline(issued.key),
                )),
            ),
            ..StorageConfig::default()
        })
    }

    #[module(imports = [presenting_storage()])]
    struct PresentingStorageHost;

    #[tokio::test]
    async fn a_client_certificate_fails_the_boot_naming_object_store_and_its_variable() {
        let Err(refused) = App::builder()
            .module::<PresentingStorageHost>()
            .build()
            .await
        else {
            panic!("a certificate object_store cannot present must not boot");
        };
        let refused = format!("{refused:#}");
        assert!(
            refused.contains(&nest_rs_config::var_name("storage", "TLS_CERT"))
                && refused.contains("object_store"),
            "{refused}"
        );
    }

    #[tokio::test]
    async fn for_root_pins_the_config_and_still_provides_the_client() {
        let app = App::builder()
            .module::<PinnedStorageHost>()
            .build()
            .await
            .expect("the pinned-config module boots");

        let cfg: Option<Arc<StorageConfig>> = app.container().get();
        assert_eq!(
            cfg.expect("pinned StorageConfig resolves").url,
            "memory:///pinned",
        );
        let storage: Option<Arc<Storage>> = app.container().get();
        assert_eq!(
            storage.expect("for_root still provides the client").url(),
            "memory:///pinned",
        );
    }
}

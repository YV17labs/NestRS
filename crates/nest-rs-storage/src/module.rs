//! Wires the shared [`Storage`] provider and its [`StorageConfig`].

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
    /// `None` ⇒ load [`StorageConfig`] from `<PREFIX>_STORAGE__*` over its
    /// defaults; `Some(cfg)` makes `cfg` the base those variables overlay.
    pub fn for_root(config: impl Into<Option<StorageConfig>>) -> StorageSetup {
        ConfigModule::setup(config)
    }
}

/// [`DynamicModule`](nest_rs_core::DynamicModule) returned by
/// [`StorageModule::for_root`]: resolves
/// [`StorageConfig`] (env over the pinned base), then brings the base
/// [`StorageModule`] wiring.
pub type StorageSetup = ConfigSetup<StorageModule, StorageConfig>;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_authn::{AUTHENTICATE_TIMEOUT, AuthError, AuthnGuard, Strategy};
    use nest_rs_core::{App, BudgetPastNetError, injectable};
    use nest_rs_http::async_trait;
    use nest_rs_http::poem::Request;

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

    #[async_trait]
    impl Strategy for StorageStrategy {
        type Principal = ();

        async fn authenticate(&self, _req: &mut Request) -> Result<(), AuthError> {
            Err(AuthError::MissingCredentials)
        }
    }

    #[module(
        imports = [StorageModule],
        providers = [StorageStrategy, AuthnGuard<StorageStrategy>],
    )]
    struct GuardedStorageModule;

    #[tokio::test]
    async fn an_operation_budget_at_the_net_of_a_guard_injecting_the_client_fails_the_boot() {
        let Err(refused) = App::builder()
            .provide(StorageConfig {
                operation_timeout: AUTHENTICATE_TIMEOUT,
                ..StorageConfig::default()
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
            .provide(StorageConfig::default())
            .module::<GuardedStorageModule>()
            .build()
            .await
            .expect("the default budget sits under every net");
    }

    /// Pinned bucket for the test below, as a real import site — the pair
    /// with it, since a pin reads no `.env` and the struct default holds none.
    fn pinned_storage() -> StorageSetup {
        StorageModule::for_root(StorageConfig {
            bucket: "pinned-bucket".into(),
            region: "eu-west-3".into(),
            access_key: "AKIAPINNED".into(),
            secret_key: "pinned-secret".into(),
            ..StorageConfig::default()
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
            cfg.expect("pinned StorageConfig resolves").bucket,
            "pinned-bucket",
        );
        let storage: Option<Arc<Storage>> = app.container().get();
        assert!(storage.is_some(), "for_root still provides the client");
    }
}

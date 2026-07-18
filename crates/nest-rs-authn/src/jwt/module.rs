//! [`AuthnModule`] — wires a configured [`JwtService`](super::JwtService) as global infrastructure.

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};

use crate::jwt::{JwtConfig, JwtService};

/// DI module that builds a [`JwtService`] from [`JwtConfig`] and provides it as
/// global infrastructure (factory phase), so any strategy or handler can inject
/// `Arc<JwtService>`.
pub struct AuthnModule;

impl AuthnModule {
    /// `None` ⇒ load [`JwtConfig`] from `NESTRS_AUTHN__*`; `Some(cfg)` pins it
    /// in code. Either way the [`JwtService`] factory is registered.
    pub fn for_root(config: impl Into<Option<JwtConfig>>) -> AuthnSetup {
        AuthnSetup {
            pinned: config.into(),
        }
    }
}

/// [`DynamicModule`] returned by [`AuthnModule::for_root`]: provides the config
/// (pinned or env-loaded), then queues the [`JwtService`] factory.
pub struct AuthnSetup {
    pinned: Option<JwtConfig>,
}

impl DynamicModule for AuthnSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        builder.provide_factory::<JwtService, _, _>(|container| async move {
            let config = container
                .get::<JwtConfig>()
                .expect("JwtConfig is resolved by ConfigModule::provide_feature");
            let options = (*config)
                .clone()
                .into_options()
                .map_err(anyhow::Error::new)?;
            JwtService::new(options).map_err(anyhow::Error::new)
        })
    }
}

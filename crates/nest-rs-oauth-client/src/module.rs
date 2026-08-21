//! [`OAuthClientModule`] — wires a configured [`OAuthClient`](crate::OAuthClient) as global infrastructure.
//!
//! For **social login** (mounting GitHub/Google or a custom provider behind an
//! open, discovered provider contract), reach for `nest-rs-social` instead —
//! its providers compose this `OAuthClient` as their shared flow. This module
//! stays for wiring a single OAuth2 client as generic infrastructure (e.g.
//! non-login API access).

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};

use crate::client::OAuthClient;
use crate::config::OAuthClientConfig;

/// DI module that provides a single configured [`OAuthClient`] as global
/// infrastructure. For social login, prefer `nest-rs-social`; this is for a lone
/// OAuth2 client (e.g. non-login API access). See the module docs.
pub struct OAuthClientModule;

impl OAuthClientModule {
    /// `None` ⇒ load [`OAuthClientConfig`] from `NESTRS_OAUTH_CLIENT__*`; `Some(cfg)` pins
    /// it in code. Either way the [`OAuthClient`] factory is registered.
    pub fn for_root(config: impl Into<Option<OAuthClientConfig>>) -> OAuthClientSetup {
        OAuthClientSetup {
            pinned: config.into(),
        }
    }
}

/// [`DynamicModule`] returned by [`OAuthClientModule::for_root`]: provides the config
/// (pinned or env-loaded), then queues the [`OAuthClient`] factory.
pub struct OAuthClientSetup {
    pinned: Option<OAuthClientConfig>,
}

impl DynamicModule for OAuthClientSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        builder.provide_factory::<OAuthClient, _, _>(|container| async move {
            let config = container
                .get::<OAuthClientConfig>()
                .expect("OAuthClientConfig is resolved by ConfigModule::provide_feature");
            OAuthClient::new((*config).clone()).map_err(anyhow::Error::new)
        })
    }
}

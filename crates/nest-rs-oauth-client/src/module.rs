//! [`OAuthClientModule`] — wires a configured [`OAuthClient`] as global infrastructure.

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::{Collecting, ContainerBuilder, Module, Registering, Unnamed};

use crate::client::OAuthClient;
use crate::config::OAuthClientConfig;

/// DI module that provides a single configured [`OAuthClient`], read bare from
/// `<PREFIX>_OAUTH__CLIENT__*`; for social login, use `nest-rs-social`.
pub struct OAuthClientModule;

impl OAuthClientModule {
    /// Pin `config` as the base `<PREFIX>_OAUTH__CLIENT__*` overlays, per
    /// field; the bare import reads those variables over
    /// [`OAuthClientConfig`]'s defaults.
    pub fn for_root(config: OAuthClientConfig) -> OAuthClientSetup {
        ConfigModule::setup(config)
    }
}

/// The configured import [`OAuthClientModule::for_root`] returns.
pub type OAuthClientSetup = ConfigSetup<OAuthClientModule, OAuthClientConfig>;

impl Module for OAuthClientModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature::<Unnamed, OAuthClientConfig>(builder)
            .provide_factory_after::<OAuthClient, (OAuthClientConfig,), _, _>(
                |container| async move {
                    let config = ConfigModule::resolved::<OAuthClientConfig>(&container)?;
                    OAuthClient::from_variables((*config).clone()).map_err(anyhow::Error::new)
                },
            )
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

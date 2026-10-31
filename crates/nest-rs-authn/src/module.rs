//! [`AuthnModule`] — wires a configured [`JwtService`] as global infrastructure.

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::{Collecting, ContainerBuilder, Module, Registering, Unnamed};

use crate::binding::ResourceBinding;
use crate::{AuthnConfig, JwtService};

/// DI module that builds a [`JwtService`] from [`AuthnConfig`] and provides it as
/// global infrastructure (factory phase), so any strategy or handler can inject
/// `Arc<JwtService>`. Imported bare, it reads `<PREFIX>_AUTHN__*`; a feature
/// verifying tokens imports it bare to declare the dependency. Under a module
/// describing the resource this app serves (`OAuthResourceModule`), the service
/// is bound to it: the resource is the audience a token must carry, and its
/// authorization servers the issuers.
pub struct AuthnModule;

impl AuthnModule {
    /// Pin `config` as the base `<PREFIX>_AUTHN__*` overlays, per field; the
    /// bare import reads those variables over [`AuthnConfig`]'s defaults.
    pub fn for_root(config: AuthnConfig) -> AuthnSetup {
        ConfigModule::setup(config)
    }
}

/// The configured import [`AuthnModule::for_root`] returns.
pub type AuthnSetup = ConfigSetup<AuthnModule, AuthnConfig>;

impl Module for AuthnModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        // A binding no import provides is waited on by nothing.
        ConfigModule::provide_feature::<Unnamed, AuthnConfig>(builder)
            .provide_factory_after::<JwtService, (AuthnConfig, ResourceBinding), _, _>(
                |container| async move {
                    let config = ConfigModule::resolved::<AuthnConfig>(&container)?;
                    let options = (*config)
                        .clone()
                        .into_options()
                        .map_err(anyhow::Error::new)?;
                    match container.get::<ResourceBinding>() {
                        Some(binding) => JwtService::bound(options, &binding),
                        None => JwtService::new(options),
                    }
                    .map_err(anyhow::Error::new)
                },
            )
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

//! [`OAuthServerModule`] — the seam that registers [`OAuthServerConfig`], the
//! clients this authorization server serves.

use std::marker::PhantomData;

use nest_rs_config::{Config, ConfigModule, ConfigSetup};
use nest_rs_core::{Collecting, ContainerBuilder, Module, Registering, Unnamed};

use crate::config::OAuthServerConfig;

/// The authorization server's registry, for an app that issues tokens: the
/// typed import `OAuthServerModule::<P>` reads [`OAuthServerConfig<P>`] from
/// `<PREFIX>_OAUTH__SERVER__*`, so a
/// [`ClientSecretBasicStrategy<P>`](crate::ClientSecretBasicStrategy) the app
/// lists authenticates against it. `P` is the payload each registered client
/// carries — the product's own type.
pub struct OAuthServerModule<P>(PhantomData<fn() -> P>);

impl<P: 'static> OAuthServerModule<P>
where
    OAuthServerConfig<P>: Config,
{
    /// Pin `config` as the base `<PREFIX>_OAUTH__SERVER__*` overlays, per
    /// field; the bare import, `OAuthServerModule::<P>`, reads those variables
    /// over [`OAuthServerConfig<P>`]'s defaults.
    pub fn for_root(config: OAuthServerConfig<P>) -> OAuthServerSetup<P> {
        ConfigModule::setup(config)
    }
}

/// The configured import [`OAuthServerModule::for_root`] returns.
pub type OAuthServerSetup<P> = ConfigSetup<OAuthServerModule<P>, OAuthServerConfig<P>>;

impl<P: 'static> Module for OAuthServerModule<P>
where
    OAuthServerConfig<P>: Config,
{
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature::<Unnamed, OAuthServerConfig<P>>(builder)
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

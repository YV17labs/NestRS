//! [`OAuthServerModule`] — the seam that registers [`OAuthServerConfig`], the
//! clients this authorization server serves, and the client authentication
//! that reads it.

use std::any::TypeId;
use std::marker::PhantomData;

use nest_rs_config::{Config, ConfigModule, ConfigSetup};
use nest_rs_core::{Collecting, ContainerBuilder, Discoverable, Module, Registering, Unnamed};

use crate::config::OAuthServerConfig;
use crate::guard::ClientGuard;
use crate::strategy::ClientSecretBasicStrategy;

/// The authorization server's registry, for an app that issues tokens: the
/// typed import `OAuthServerModule::<P>` reads [`OAuthServerConfig<P>`] from
/// `<PREFIX>_OAUTH__SERVER__*` and provides what authenticates a client
/// against it — [`ClientSecretBasicStrategy<P>`] and its guard,
/// [`ClientGuard<P>`], which a token endpoint binds with
/// `#[use_guards(ClientGuard<P>)]`. `P` is the payload each registered client
/// carries — the product's own type.
///
/// A module listing the strategy or the guard too registers it twice, which
/// fails the boot naming both registrations.
pub struct OAuthServerModule<P>(PhantomData<fn() -> P>);

impl<P: Clone + Send + Sync + 'static> OAuthServerModule<P>
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

impl<P: Clone + Send + Sync + 'static> Module for OAuthServerModule<P>
where
    OAuthServerConfig<P>: Config,
{
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature::<Unnamed, OAuthServerConfig<P>>(builder)
    }

    // By hand: a `#[module]` is not generic, and the strategy is built from
    // the registry its config's factory already resolved.
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        let builder = provide::<ClientSecretBasicStrategy<P>>(builder);
        provide::<ClientGuard<P>>(builder)
    }
}

/// Register `T` as `#[module]` registers a provider it lists: one a test
/// overrides is claimed by its double and never built.
fn provide<T: Discoverable + 'static>(builder: ContainerBuilder) -> ContainerBuilder {
    if nest_rs_core::__private::overrides_any(&builder, &[TypeId::of::<T>()]) {
        T::__nestrs_register_overridden(builder)
    } else {
        T::register(builder)
    }
}

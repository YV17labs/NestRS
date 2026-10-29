//! [`OAuthServerModule`] — the seam that registers [`OAuthServerConfig`], the
//! clients this authorization server serves.

use std::any::TypeId;

use nest_rs_config::{Config, ConfigModule};
use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule};

use crate::config::OAuthServerConfig;

/// The authorization server's registry, for an app that issues tokens:
/// `OAuthServerModule::for_root::<P>(None)` reads [`OAuthServerConfig<P>`]
/// from `<PREFIX>_OAUTH__SERVER__*`, so a
/// [`ClientSecretBasicStrategy<P>`](crate::ClientSecretBasicStrategy) the app
/// lists authenticates against it. `P` is the payload each registered client
/// carries — the product's own type.
pub struct OAuthServerModule;

impl OAuthServerModule {
    /// `None` loads [`OAuthServerConfig<P>`] from `<PREFIX>_OAUTH__SERVER__*`;
    /// `Some(cfg)` makes `cfg` the base those variables overlay, per field.
    pub fn for_root<P>(config: impl Into<Option<OAuthServerConfig<P>>>) -> OAuthServerSetup<P>
    where
        OAuthServerConfig<P>: Config,
    {
        OAuthServerSetup {
            pinned: config.into(),
        }
    }
}

/// [`DynamicModule`] returned by [`OAuthServerModule::for_root`].
pub struct OAuthServerSetup<P> {
    pinned: Option<OAuthServerConfig<P>>,
}

impl<P: 'static> DynamicModule for OAuthServerSetup<P>
where
    OAuthServerConfig<P>: Config,
{
    fn module() -> TypeId {
        TypeId::of::<OAuthServerModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature(self.pinned.clone(), builder)
    }
}

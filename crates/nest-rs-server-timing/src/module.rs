use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::module;

use crate::config::ServerTimingConfig;
use crate::interceptor::ServerTiming;

/// Add to `#[module(imports = [...])]` to attach the `Server-Timing` header to
/// every response but a `401`, `403`, `407` or `429` — in the development and
/// test profiles, and elsewhere once [`ServerTimingConfig::enabled`] says so.
///
/// ```
/// use nest_rs_core::module;
/// use nest_rs_server_timing::{ServerTimingConfig, ServerTimingModule};
///
/// #[module(imports = [ServerTimingModule])]
/// struct ApiModule;
///
/// #[module(imports = [ServerTimingModule::for_root(ServerTimingConfig { enabled: true })])]
/// struct EverywhereModule;
/// ```
#[module(
    imports = [ConfigModule::for_feature::<ServerTimingConfig>()],
    providers = [ServerTiming],
)]
pub struct ServerTimingModule;

impl ServerTimingModule {
    /// `None` ⇒ load [`ServerTimingConfig`] from `<PREFIX>_SERVER_TIMING__*`
    /// over its profile's defaults; `Some(cfg)` makes `cfg` the base those
    /// variables overlay.
    pub fn for_root(config: impl Into<Option<ServerTimingConfig>>) -> ServerTimingSetup {
        ConfigModule::setup(config)
    }
}

/// [`DynamicModule`](nest_rs_core::DynamicModule) returned by
/// [`ServerTimingModule::for_root`].
pub type ServerTimingSetup = ConfigSetup<ServerTimingModule, ServerTimingConfig>;

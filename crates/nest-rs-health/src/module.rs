//! [`HealthModule`] — mounts the probe routes, resolves [`HealthConfig`], and
//! fills the indicator registry at the kernel's wiring step.

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::{Container, ContainerBuilder, Module, Registering, module};

use crate::config::HealthConfig;
use crate::controller::HealthController;
use crate::service::HealthService;

#[module(
    imports = [ConfigModule::for_feature::<HealthConfig>(), HealthWiring],
    providers = [
        HealthService,
        HealthController,
    ],
)]
/// Provides the health probe endpoints. Import it to mount `/health` and let
/// any reachable provider contribute `#[liveness]`/`#[readiness]`/`#[startup]`
/// indicators.
pub struct HealthModule;

impl HealthModule {
    /// `None` ⇒ load [`HealthConfig`] from `<PREFIX>_HEALTH__*` over its defaults;
    /// `Some(cfg)` makes `cfg` the base those variables overlay.
    pub fn for_root(config: impl Into<Option<HealthConfig>>) -> HealthSetup {
        ConfigModule::setup(config)
    }
}

/// [`DynamicModule`](nest_rs_core::DynamicModule) returned by
/// [`HealthModule::for_root`].
pub type HealthSetup = ConfigSetup<HealthModule, HealthConfig>;

/// The registry [`HealthModule`]'s wiring fills, as a boot failure names it.
const INDICATORS: &str = "nest_rs::health::indicators";

/// Attaches the indicator wiring: `#[module]` takes only imports and providers,
/// so this hand-written module carries the step for [`HealthModule`].
struct HealthWiring;

impl Module for HealthWiring {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_wiring(INDICATORS, wire_indicators)
    }
}

/// Hand the service the assembled container, once the app's indicators are
/// known to answer under distinct names.
fn wire_indicators(container: &Container) -> anyhow::Result<()> {
    if let Some(svc) = container.get::<HealthService>() {
        // The report's fold by name would silently keep one of two verdicts.
        crate::service::check_indicator_names(container)?;
        svc.install_container(container.clone());
    }
    Ok(())
}

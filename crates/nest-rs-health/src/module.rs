//! [`HealthModule`] — mounts the probe routes, resolves [`HealthConfig`],
//! fills the indicator registry at the kernel's wiring step, and hands the
//! service's container back at its close step.

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::{Container, ContainerBuilder, Module, ReachableProviders, Registering, module};

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
/// indicators, and any imported module attach an
/// [`IndicatorEntry`](crate::IndicatorEntry).
pub struct HealthModule;

impl HealthModule {
    /// `None` ⇒ load [`HealthConfig`] from `<PREFIX>_HEALTH__*` over its defaults;
    /// `Some(cfg)` makes `cfg` the base those variables overlay.
    pub fn for_root(config: impl Into<Option<HealthConfig>>) -> HealthSetup {
        ConfigModule::setup(config.into())
    }
}

/// [`DynamicModule`](nest_rs_core::DynamicModule) returned by
/// [`HealthModule::for_root`].
pub type HealthSetup = ConfigSetup<HealthModule, HealthConfig>;

/// Attaches the indicator wiring and the close that undoes it: `#[module]`
/// takes only imports and providers, so this hand-written module carries the
/// steps for [`HealthModule`].
struct HealthWiring;

impl Module for HealthWiring {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide_wiring("nest_rs::health::indicators", wire_indicators)
            .provide_close::<HealthService, _, _>(|svc| async move {
                svc.release_container();
                Ok(())
            })
    }
}

/// Hand the service the assembled container and the indicators it runs — the
/// decorated methods and the entries modules attached — once they are known
/// to answer under distinct names.
fn wire_indicators(container: &Container) -> anyhow::Result<()> {
    if let Some(svc) = container.get::<HealthService>() {
        let reachable = ReachableProviders::of(container, crate::service::INDICATORS)?;
        let indicators = crate::service::wired(container, &reachable);
        // The report's fold by name would silently keep one of two verdicts.
        crate::service::check_names(&indicators)?;
        svc.install(container, &reachable, indicators);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indicators_wired_over_a_container_no_app_sealed_are_refused_naming_their_reader() {
        let by_hand = Container::builder()
            .provide(HealthService::default())
            .build()
            .expect("the container builds");

        let Err(refused) = wire_indicators(&by_hand) else {
            panic!("an ungated discovery is refused, never served whole");
        };

        let ungated = refused
            .downcast_ref::<nest_rs_core::UngatedDiscoveryError>()
            .unwrap_or_else(|| panic!("not the ungated refusal: {refused:#}"));
        assert_eq!(ungated.reader, "indicator");
    }
}

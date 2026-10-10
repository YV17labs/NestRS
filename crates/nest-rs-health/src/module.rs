//! [`HealthModule`] — mounts the probe routes and resolves [`HealthConfig`].

use std::future::Future;
use std::pin::Pin;

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::__private::LifecycleHook;
use nest_rs_core::{Container, LifecyclePhase, module};

use crate::config::HealthConfig;
use crate::controller::HealthController;
use crate::service::HealthService;

#[module(
    imports = [ConfigModule::for_feature::<HealthConfig>()],
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

// Self-gates inside `install_container` on the service being present, hence
// `present: |_| true`.
nest_rs_core::inventory::submit! {
    LifecycleHook {
        phase: LifecyclePhase::OnApplicationBootstrap,
        provider: "HealthModule",
        method: "install_container",
        origin: module_path!(),
        provider_type_id: std::any::TypeId::of::<HealthModule>,
        present: |_| true,
        run: install_container,
    }
}

fn install_container(
    container: &Container,
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
    Box::pin(async move {
        if let Some(svc) = container.get::<HealthService>() {
            // The report's fold by name would silently keep one of two verdicts.
            crate::service::check_indicator_names(container)?;
            svc.install_container(container.clone());
        }
        Ok(())
    })
}

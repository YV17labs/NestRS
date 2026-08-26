//! [`HealthModule`] — mounts the probe routes and resolves [`HealthConfig`].
//!
//! [`HealthConfig`] loads from `NESTRS_HEALTH__*` by default (importing
//! `HealthModule` is enough); [`HealthModule::for_root`] supplies a base for
//! those variables to overlay, so a ceiling pinned in code is still overridable
//! per field by the deployment (see `nest_rs_config::Config`).

use std::future::Future;
use std::pin::Pin;

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::{Container, LifecycleHook, LifecyclePhase, module};

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
    /// `None` ⇒ load [`HealthConfig`] from `NESTRS_HEALTH__*` over its defaults;
    /// `Some(cfg)` makes `cfg` the base those variables overlay. Either way the
    /// probe routes are mounted, so this is a drop-in replacement for importing
    /// the bare [`HealthModule`].
    pub fn for_root(config: impl Into<Option<HealthConfig>>) -> HealthSetup {
        ConfigModule::setup(config)
    }
}

/// [`DynamicModule`](nest_rs_core::DynamicModule) returned by
/// [`HealthModule::for_root`]: resolves [`HealthConfig`] (env over the pinned
/// base), then brings the base [`HealthModule`] wiring (the service and the
/// controller). Pin-and-recurse is the whole behaviour, so it is a
/// [`ConfigSetup`] rather than a type of its own.
pub type HealthSetup = ConfigSetup<HealthModule, HealthConfig>;

// Stash the assembled container on `HealthService` so its `probe()` can
// resolve indicator providers at request time. The `EventsModule` uses the
// same lifecycle-hook seam to wire its discovered handlers — see
// `crates/nest-rs-events/src/module.rs`.
// Infra hook self-gates inside `install_container` (no-op when the service is
// absent), so it opts out of the inert-hook warn with `present: |_| true`.
nest_rs_core::inventory::submit! {
    LifecycleHook {
        phase: LifecyclePhase::OnApplicationBootstrap,
        provider: "HealthModule",
        method: "install_container",
        origin: module_path!(),
        present: |_| true,
        run: install_container,
    }
}

fn install_container(
    container: &Container,
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
    Box::pin(async move {
        if let Some(svc) = container.get::<HealthService>() {
            // Before the registry is put to work: two reachable indicators
            // claiming one name on one probe fail the boot naming both hosts,
            // because the fold that follows would silently keep one verdict.
            crate::service::check_indicator_names(container)?;
            svc.install_container(container.clone());
        }
        Ok(())
    })
}

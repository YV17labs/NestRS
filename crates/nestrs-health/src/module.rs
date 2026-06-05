use std::future::Future;
use std::pin::Pin;

use nestrs_core::{Container, LifecycleHook, LifecyclePhase, module};

use crate::controller::HealthController;
use crate::service::HealthService;

#[module(
    providers = [
        HealthService,
        HealthController,
    ],
)]
pub struct HealthModule;

// Stash the assembled container on `HealthService` so its `probe()` can
// resolve indicator providers at request time. The `EventModule` uses the
// same lifecycle-hook seam to wire its discovered handlers — see
// `crates/nestrs-events/src/module.rs`.
nestrs_core::inventory::submit! {
    LifecycleHook {
        phase: LifecyclePhase::OnApplicationBootstrap,
        provider: "HealthModule",
        method: "install_container",
        run: install_container,
    }
}

fn install_container(
    container: &Container,
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
    Box::pin(async move {
        if let Some(svc) = container.get::<HealthService>() {
            svc.install_container(container.clone());
        }
        Ok(())
    })
}

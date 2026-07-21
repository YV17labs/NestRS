//! [`SocialModule`] — the module that owns the social provider registry, and
//! the only import a social login needs. It provides [`SocialRegistry`]; at
//! bootstrap the registry activates every linked provider whose credentials
//! are configured.

use std::future::Future;
use std::pin::Pin;

use nest_rs_core::{Container, LifecycleHook, LifecyclePhase, module};

use crate::registry::SocialRegistry;

/// Provides the [`SocialRegistry`]. Import it once so every linked, configured
/// social provider is discovered and validated at boot.
#[module(providers = [SocialRegistry])]
pub struct SocialModule;

// Resolve + validate the configured providers once the container is assembled,
// then stash the map on `SocialRegistry`. Same lifecycle-hook seam as
// `HealthModule::install_container`. Self-gates on the service being present,
// so it opts out of the inert-hook warn with `present: |_| true`.
nest_rs_core::inventory::submit! {
    LifecycleHook {
        phase: LifecyclePhase::OnApplicationBootstrap,
        provider: "SocialModule",
        method: "install",
        present: |_| true,
        run: install,
    }
}

fn install(container: &Container) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
    Box::pin(async move {
        match container.get::<SocialRegistry>() {
            Some(providers) => providers.install(container),
            None => Ok(()),
        }
    })
}

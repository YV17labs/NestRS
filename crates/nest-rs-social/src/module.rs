//! [`SocialModule`] — the base module. Importing it provides
//! [`SocialProviders`]; importing a provider module (e.g.
//! `GithubSocialProviderModule`) makes that provider reachable so the registry picks
//! it up at bootstrap.

use std::future::Future;
use std::pin::Pin;

use nest_rs_core::{Container, LifecycleHook, LifecyclePhase, module};

use crate::registry::SocialProviders;

#[module(providers = [SocialProviders])]
pub struct SocialModule;

// Resolve + validate the reachable providers once the container is assembled,
// then stash the map on `SocialProviders`. Same lifecycle-hook seam as
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

fn install(
    container: &Container,
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
    Box::pin(async move {
        match container.get::<SocialProviders>() {
            Some(providers) => providers.install(container),
            None => Ok(()),
        }
    })
}

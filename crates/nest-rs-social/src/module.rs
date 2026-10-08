//! [`SocialModule`] — the module that owns the social provider registry, and
//! the only import a social login needs. It provides [`SocialRegistry`]; at
//! bootstrap the registry activates every linked provider whose credentials
//! are configured.
//!
//! It takes no configuration: each provider reads its own namespace, and a
//! hermetic test seeds that config on the builder.

use std::future::Future;
use std::pin::Pin;

use nest_rs_core::{Container, LifecycleHook, LifecyclePhase, module};

use crate::registry::SocialRegistry;

/// Provides the [`SocialRegistry`]. Import it once so every linked, configured
/// social provider is discovered and validated at boot.
#[module(providers = [SocialRegistry])]
pub struct SocialModule;

// Self-gates on the registry being present, hence `present: |_| true`.
nest_rs_core::inventory::submit! {
    LifecycleHook {
        phase: LifecyclePhase::OnApplicationBootstrap,
        provider: "SocialModule",
        method: "install",
        origin: module_path!(),
        provider_type_id: std::any::TypeId::of::<SocialModule>,
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

#[cfg(test)]
mod tests {
    use nest_rs_core::App;

    use super::*;
    use crate::providers::github::GithubSocialConfig;

    /// Importing `SocialModule` discovers both entries: the seeded GitHub
    /// activates, the unconfigured Google stays inert. Drives `install` directly,
    /// as `App::build` stops before the lifecycle phases.
    #[tokio::test]
    async fn discovery_configures_each_provider_from_its_own_config() {
        let app = App::builder()
            .module::<SocialModule>()
            .provide(GithubSocialConfig {
                client_id: "seeded-client".into(),
                client_secret: "seeded-secret".into(),
                redirect_url: "https://acme.example.com/auth/github/callback".into(),
                scopes: Vec::new(),
            })
            .build()
            .await
            .expect("a bare SocialModule boots");

        install(app.container())
            .await
            .expect("both entries resolve: one configured, one inert");

        let registry = app
            .container()
            .get::<SocialRegistry>()
            .expect("SocialModule provides the registry");

        assert_eq!(
            registry.keys(),
            ["github"],
            "the configured provider activates and the unconfigured one stays inert",
        );
    }
}

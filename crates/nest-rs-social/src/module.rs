//! [`SocialModule`] — the module that owns the social provider registry, and
//! the only import a social login needs. It provides [`SocialRegistry`]; the
//! boot's wiring step, before the first lifecycle hook, activates every linked
//! provider whose credentials are configured.
//!
//! It takes no configuration: each provider reads its own namespace, and a
//! hermetic test seeds that config on the builder.

use nest_rs_core::{Container, ContainerBuilder, Module, Registering, module};

use crate::registry::SocialRegistry;

/// Provides the [`SocialRegistry`]. Import it once so every linked, configured
/// social provider is discovered and validated at boot.
#[module(imports = [SocialWiring], providers = [SocialRegistry])]
pub struct SocialModule;

/// Attaches the provider wiring: `#[module]` takes only imports and providers,
/// so this hand-written module carries the step for [`SocialModule`].
struct SocialWiring;

impl Module for SocialWiring {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_wiring("nest_rs::social::providers", install)
    }
}

fn install(container: &Container) -> anyhow::Result<()> {
    match container.get::<SocialRegistry>() {
        Some(providers) => providers.install(container),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::App;

    use super::*;
    use crate::providers::github::GithubSocialConfig;

    /// Importing `SocialModule` discovers both entries: the seeded GitHub
    /// activates, the unconfigured Google stays inert — by the time the boot
    /// returns.
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
            .expect("a bare SocialModule boots: one entry configured, one inert");

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

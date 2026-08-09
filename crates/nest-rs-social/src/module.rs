//! [`SocialModule`] — the module that owns the social provider registry, and
//! the only import a social login needs. It provides [`SocialRegistry`]; at
//! bootstrap the registry activates every linked provider whose credentials
//! are configured.
//!
//! **It takes no configuration, and that is the design.** A provider is
//! discovered from the link-time registry, and it carries its *own* `#[config]`
//! — `GithubSocialConfig` is namespaced `social__github`, so the entry loads
//! `NESTRS_SOCIAL__GITHUB__*` itself when the registry builds it. `SocialModule`
//! never learns which providers exist, so it has nothing to be configured
//! *about*: a `for_root` here would be a module declaring config it does not
//! own, and would have to erase the type of every provider's unrelated config
//! to carry them in one list.
//!
//! A provider's credentials are therefore **deployment data, not code**: they
//! come from that provider's namespace, and a test that must not read the
//! ambient environment seeds the config on the builder. There is nothing to pin
//! in a module import. See [`resolve_provider`](crate::resolve_provider) for the
//! resolution order.

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
        origin: module_path!(),
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

    /// The composition witness, and the reason this module has no `for_root`:
    /// the app declares *nothing* about which providers exist. Importing
    /// `SocialModule` discovers both first-party entries, and each one reaches
    /// for its own config — GitHub's is supplied, so GitHub activates; Google's
    /// namespace is set nowhere, so Google stays inert instead of failing boot.
    ///
    /// Seeds the credentials rather than reading the ambient environment: a
    /// provider's config is deployment data, so a seed is the documented way to
    /// make a test hermetic against it.
    ///
    /// Drives `install` directly: it is the body the `OnApplicationBootstrap`
    /// hook above runs, and `App::build` stops before the lifecycle phases.
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

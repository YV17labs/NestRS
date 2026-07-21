use nest_rs_authn::OAuth2Client;
use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};

use super::config::GithubSocialConfig;
use super::provider::GithubSocialProvider;

/// Wires the GitHub provider. Always imported with
/// `GithubSocialProviderModule::for_root(...)`: it registers
/// [`GithubSocialConfig`] (env or pinned) and a factory that builds the
/// [`GithubSocialProvider`] — invalid config **fails boot naming the
/// provider**. Not importing it leaves the registered
/// [`SocialProviderEntry`](crate::SocialProviderEntry) inert.
pub struct GithubSocialProviderModule;

impl GithubSocialProviderModule {
    /// Pass `None` to load [`GithubSocialConfig`] from
    /// `NESTRS_SOCIAL__GITHUB__*`, or a `GithubSocialConfig` to pin it in code
    /// (wins over the environment).
    pub fn for_root(config: impl Into<Option<GithubSocialConfig>>) -> GithubSocialProviderSetup {
        GithubSocialProviderSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`GithubSocialProviderModule::for_root`].
pub struct GithubSocialProviderSetup {
    pinned: Option<GithubSocialConfig>,
}

impl DynamicModule for GithubSocialProviderSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        builder.provide_factory::<GithubSocialProvider, _, _>(|container| async move {
            let config = container
                .get::<GithubSocialConfig>()
                .expect("GithubSocialConfig is resolved by ConfigModule::provide_feature");
            let client = OAuth2Client::new(config.oauth2_config())
                .map_err(|e| anyhow::anyhow!("invalid GitHub social provider config: {e}"))?;
            Ok(GithubSocialProvider::new(client))
        })
    }
}

use nest_rs_authn::OAuth2Client;
use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};

use super::config::GoogleSocialConfig;
use super::provider::GoogleSocialProvider;

/// Wires the Google provider — same shape as
/// [`GithubSocialProviderModule`](super::super::github::GithubSocialProviderModule).
pub struct GoogleSocialProviderModule;

impl GoogleSocialProviderModule {
    /// `None` loads [`GoogleSocialConfig`] from `NESTRS_SOCIAL__GOOGLE__*`;
    /// `Some(cfg)` pins it in code.
    pub fn for_root(config: impl Into<Option<GoogleSocialConfig>>) -> GoogleSocialProviderSetup {
        GoogleSocialProviderSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`GoogleSocialProviderModule::for_root`].
pub struct GoogleSocialProviderSetup {
    pinned: Option<GoogleSocialConfig>,
}

impl DynamicModule for GoogleSocialProviderSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        builder.provide_factory::<GoogleSocialProvider, _, _>(|container| async move {
            let config = container
                .get::<GoogleSocialConfig>()
                .expect("GoogleSocialConfig is resolved by ConfigModule::provide_feature");
            let client = OAuth2Client::new(config.oauth2_config())
                .map_err(|e| anyhow::anyhow!("invalid Google social provider config: {e}"))?;
            Ok(GoogleSocialProvider::new(client))
        })
    }
}

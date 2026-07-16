use nest_rs_authn::OAuth2Client;
use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};

use super::config::GoogleSocialConfig;
use super::provider::GoogleSocialProvider;

/// Wires the Google provider — same shape as
/// [`GithubSocialProviderModule`](super::super::github::GithubSocialProviderModule).
#[derive(Default)]
pub struct GoogleSocialProviderModule {
    pinned: Option<GoogleSocialConfig>,
}

impl GoogleSocialProviderModule {
    pub fn for_root(config: impl Into<Option<GoogleSocialConfig>>) -> Self {
        Self {
            pinned: config.into(),
        }
    }
}

impl DynamicModule for GoogleSocialProviderModule {
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

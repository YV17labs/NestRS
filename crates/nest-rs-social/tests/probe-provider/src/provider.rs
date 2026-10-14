use nest_rs::config::Namespaced;
use nest_rs::oauth::client::{OAuthClient, TokenSet};
use nest_rs::social::{
    ProfileFuture, SocialProfile, SocialProvider, SocialProviderEntry, resolve_provider,
};

use crate::config::ProbeSocialConfig;
use crate::module::ProbeSocialModule;

/// The probe provider; its profile is a constant subject.
pub struct ProbeSocialProvider {
    client: OAuthClient,
}

impl ProbeSocialProvider {
    /// The key the probe registers under.
    pub const KEY: &'static str = "probe";
}

impl SocialProvider for ProbeSocialProvider {
    fn key(&self) -> &'static str {
        Self::KEY
    }

    fn client(&self) -> &OAuthClient {
        &self.client
    }

    fn profile<'a>(&'a self, _tokens: &'a TokenSet) -> ProfileFuture<'a> {
        Box::pin(async { Ok(SocialProfile::new(Self::KEY, "probe-subject")) })
    }
}

nest_rs::core::inventory::submit! {
    SocialProviderEntry {
        key: ProbeSocialProvider::KEY,
        provider_type_name: || std::any::type_name::<ProbeSocialProvider>(),
        config_namespace: <ProbeSocialConfig as Namespaced>::NAMESPACE,
        build: |container| {
            resolve_provider::<ProbeSocialProvider, ProbeSocialConfig>(container, |config| {
                Ok(ProbeSocialProvider {
                    client: OAuthClient::new(config.oauth2_config())?,
                })
            })
        },
        module: || std::any::TypeId::of::<ProbeSocialModule>(),
    }
}

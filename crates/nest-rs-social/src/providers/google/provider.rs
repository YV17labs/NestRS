use nest_rs_authn::{AuthError, OAuth2Client, TokenSet};
use serde::Deserialize;

use super::config::GoogleSocialConfig;
use crate::provider::{ProfileFuture, SocialProfile, SocialProvider};
use crate::registry::{SocialProviderEntry, resolve_provider};

/// The Google OIDC social provider. Reads the profile from the userinfo
/// endpoint with the access token; overrides only [`SocialProvider::profile`].
/// (Reading identity from the id_token instead is a future optimization — the
/// userinfo path keeps Google on the zero-override flow template.)
pub struct GoogleSocialProvider {
    client: OAuth2Client,
}

impl GoogleSocialProvider {
    pub(crate) const KEY: &'static str = "google";

    pub(crate) fn new(client: OAuth2Client) -> Self {
        Self { client }
    }
}

#[derive(Debug, Deserialize)]
struct GoogleUserinfo {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
    #[serde(default)]
    name: Option<String>,
}

impl SocialProvider for GoogleSocialProvider {
    fn key(&self) -> &'static str {
        Self::KEY
    }

    fn client(&self) -> &OAuth2Client {
        &self.client
    }

    fn profile<'a>(&'a self, tokens: &'a TokenSet) -> ProfileFuture<'a> {
        Box::pin(async move {
            let info: GoogleUserinfo = self.client.userinfo(&tokens.access_token).await?;
            Ok::<_, AuthError>(
                SocialProfile::new(Self::KEY, info.sub)
                    .with_email(info.email, info.email_verified)
                    .with_name(info.name),
            )
        })
    }
}

nest_rs_core::inventory::submit! {
    SocialProviderEntry {
        key: GoogleSocialProvider::KEY,
        provider_type_name: || std::any::type_name::<GoogleSocialProvider>(),
        config_namespace: <GoogleSocialConfig as nest_rs_config::Namespaced>::NAMESPACE,
        build: |container| {
            resolve_provider::<GoogleSocialProvider, GoogleSocialConfig>(container, |config| {
                let client = OAuth2Client::new(config.oauth2_config())
                    .map_err(|e| anyhow::anyhow!("invalid Google OAuth2 client config: {e}"))?;
                Ok(GoogleSocialProvider::new(client))
            })
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> GoogleUserinfo {
        serde_json::from_str(json).expect("valid userinfo json")
    }

    #[test]
    fn verified_email_passes_through_as_verified() {
        let info =
            parse(r#"{"sub":"1","email":"ada@example.com","email_verified":true,"name":"Ada"}"#);
        let profile = SocialProfile::new(GoogleSocialProvider::KEY, info.sub)
            .with_email(info.email, info.email_verified)
            .with_name(info.name);
        assert_eq!(profile.subject, "1");
        assert_eq!(profile.email.as_deref(), Some("ada@example.com"));
        assert!(profile.email_verified);
    }

    #[test]
    fn unverified_email_is_reported_unverified() {
        let info = parse(r#"{"sub":"2","email":"eve@example.com","email_verified":false}"#);
        let profile = SocialProfile::new(GoogleSocialProvider::KEY, info.sub)
            .with_email(info.email, info.email_verified);
        assert!(
            !profile.email_verified,
            "an unverified google email must never link"
        );
    }

    #[test]
    fn missing_email_verified_defaults_to_false() {
        let info = parse(r#"{"sub":"3","email":"x@example.com"}"#);
        assert!(!info.email_verified);
    }
}

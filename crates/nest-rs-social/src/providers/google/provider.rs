use nest_rs_authn::AuthError;
use nest_rs_core::Secret;
use nest_rs_oauth_client::{OAuthClient, TokenSet};
use serde::Deserialize;

use super::config::GoogleSocialConfig;
use super::module::GoogleSocialModule;
use crate::provider::{ProfileFuture, SocialProfile, SocialProvider};
use crate::registry::{SocialProviderEntry, resolve_provider};

/// The Google OIDC social provider, reading the profile from the userinfo
/// endpoint; overrides only [`SocialProvider::profile`].
pub struct GoogleSocialProvider {
    client: OAuthClient,
}

impl GoogleSocialProvider {
    pub(crate) const KEY: &'static str = "google";

    pub(crate) fn new(client: OAuthClient) -> Self {
        Self { client }
    }
}

#[derive(Debug, Deserialize)]
struct GoogleUserinfo {
    sub: String,
    #[serde(default)]
    email: Option<Secret<String>>,
    #[serde(default)]
    email_verified: bool,
    #[serde(default)]
    name: Option<Secret<String>>,
}

impl GoogleUserinfo {
    /// The normalized profile, the one place the personal data is read.
    fn into_profile(self) -> SocialProfile {
        let exposed =
            |value: Option<Secret<String>>| value.map(|value| value.expose_secret().clone());
        SocialProfile::new(GoogleSocialProvider::KEY, self.sub)
            .with_email(exposed(self.email), self.email_verified)
            .with_name(exposed(self.name))
    }
}

impl SocialProvider for GoogleSocialProvider {
    fn key(&self) -> &'static str {
        Self::KEY
    }

    fn client(&self) -> &OAuthClient {
        &self.client
    }

    fn profile<'a>(&'a self, tokens: &'a TokenSet) -> ProfileFuture<'a> {
        Box::pin(async move {
            let info: GoogleUserinfo = self
                .client
                .userinfo(tokens.access_token.expose_secret())
                .await?;
            Ok::<_, AuthError>(info.into_profile())
        })
    }
}

nest_rs_core::inventory::submit! {
    SocialProviderEntry::new::<GoogleSocialProvider, GoogleSocialModule>(
        GoogleSocialProvider::KEY,
        <GoogleSocialConfig as nest_rs_config::Namespaced>::NAMESPACE,
        |container| {
            resolve_provider::<GoogleSocialProvider, GoogleSocialConfig>(container, |config| {
                let client = OAuthClient::new(config.oauth2_config()?)
                    .map_err(|e| anyhow::Error::new(e).context("invalid Google OAuth2 client config"))?;
                Ok(GoogleSocialProvider::new(client))
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> GoogleUserinfo {
        serde_json::from_str(json).expect("valid userinfo json")
    }

    /// The account's email and name are personal data: the DTO read off the
    /// userinfo answer prints whether each is there, never what.
    #[test]
    fn debug_never_prints_the_personal_data_google_answers() {
        let info = parse(
            r#"{"sub":"1","email":"ada@example.com","email_verified":true,"name":"Ada Lovelace"}"#,
        );

        let printed = format!("{info:?}");

        for personal in ["ada@example.com", "Lovelace"] {
            assert!(!printed.contains(personal), "{printed}");
        }
        assert!(
            printed.contains("sub: \"1\""),
            "the subject stays: {printed}"
        );
    }

    #[test]
    fn verified_email_passes_through_as_verified() {
        let profile =
            parse(r#"{"sub":"1","email":"ada@example.com","email_verified":true,"name":"Ada"}"#)
                .into_profile();
        assert_eq!(profile.subject, "1");
        assert_eq!(
            profile
                .email
                .as_ref()
                .map(|email| email.expose_secret().as_str()),
            Some("ada@example.com")
        );
        assert!(profile.email_verified);
    }

    #[test]
    fn unverified_email_is_reported_unverified() {
        let profile =
            parse(r#"{"sub":"2","email":"eve@example.com","email_verified":false}"#).into_profile();
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

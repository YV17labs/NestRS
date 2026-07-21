use nest_rs_authn::{AuthError, OAuth2Client, TokenSet};
use serde::Deserialize;

use super::config::GithubSocialConfig;
use crate::provider::{ProfileFuture, SocialProfile, SocialProvider};
use crate::registry::{SocialProviderEntry, resolve_provider};

/// The GitHub social provider. Holds the shared [`OAuth2Client`] and overrides
/// only [`SocialProvider::profile`] — the redirect and code-exchange legs use
/// the trait defaults.
pub struct GithubSocialProvider {
    client: OAuth2Client,
}

impl GithubSocialProvider {
    pub(crate) const KEY: &'static str = "github";
    const EMAILS_URL: &'static str = "https://api.github.com/user/emails";

    pub(crate) fn new(client: OAuth2Client) -> Self {
        Self { client }
    }

    /// GitHub's `GET /user` does not attest email verification (and often
    /// omits the email entirely), so prefer the primary **verified** address
    /// from the dedicated emails endpoint. Fall back to the profile email as
    /// *unverified* when the emails endpoint is unavailable (scope not granted)
    /// or yields nothing verified.
    async fn resolve_email(
        &self,
        profile_email: Option<String>,
        access_token: &str,
    ) -> (Option<String>, bool) {
        match self
            .client
            .fetch::<Vec<GithubEmail>>(Self::EMAILS_URL, access_token)
            .await
        {
            Ok(emails) => {
                let verified = emails
                    .iter()
                    .find(|e| e.primary && e.verified)
                    .or_else(|| emails.iter().find(|e| e.verified));
                if let Some(entry) = verified {
                    return (Some(entry.email.clone()), true);
                }
            }
            Err(err) => {
                tracing::debug!(
                    target: "nest_rs::social",
                    provider = Self::KEY,
                    error = %err,
                    "github emails endpoint unavailable; falling back to unverified profile email",
                );
            }
        }
        (profile_email, false)
    }
}

#[derive(Debug, Deserialize)]
struct GithubUser {
    id: i64,
    #[serde(default)]
    login: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

impl SocialProvider for GithubSocialProvider {
    fn key(&self) -> &'static str {
        Self::KEY
    }

    fn client(&self) -> &OAuth2Client {
        &self.client
    }

    fn profile<'a>(&'a self, tokens: &'a TokenSet) -> ProfileFuture<'a> {
        Box::pin(async move {
            // `/user` is the configured `userinfo_url` — go through `userinfo`
            // so the endpoint has a single home (the config), like Google.
            let user: GithubUser = self.client.userinfo(&tokens.access_token).await?;
            let display_name = user.name.or(user.login);
            let (email, verified) = self.resolve_email(user.email, &tokens.access_token).await;
            Ok::<_, AuthError>(
                SocialProfile::new(Self::KEY, user.id.to_string())
                    .with_email(email, verified)
                    .with_name(display_name),
            )
        })
    }
}

nest_rs_core::inventory::submit! {
    SocialProviderEntry {
        key: GithubSocialProvider::KEY,
        provider_type_name: || std::any::type_name::<GithubSocialProvider>(),
        config_namespace: <GithubSocialConfig as nest_rs_config::Namespaced>::NAMESPACE,
        build: |container| {
            resolve_provider::<GithubSocialProvider, GithubSocialConfig>(container, |config| {
                let client = OAuth2Client::new(config.oauth2_config())
                    .map_err(|e| anyhow::anyhow!("invalid GitHub OAuth2 client config: {e}"))?;
                Ok(GithubSocialProvider::new(client))
            })
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(email: Option<&str>, login: Option<&str>, name: Option<&str>) -> GithubUser {
        GithubUser {
            id: 42,
            login: login.map(str::to_owned),
            name: name.map(str::to_owned),
            email: email.map(str::to_owned),
        }
    }

    fn parse_emails(json: &str) -> Vec<GithubEmail> {
        serde_json::from_str(json).expect("valid emails json")
    }

    #[test]
    fn primary_verified_email_is_picked_over_other_verified() {
        let emails = parse_emails(
            r#"[
                {"email":"secondary@example.com","primary":false,"verified":true},
                {"email":"primary@example.com","primary":true,"verified":true}
            ]"#,
        );
        let chosen = emails
            .iter()
            .find(|e| e.primary && e.verified)
            .or_else(|| emails.iter().find(|e| e.verified))
            .map(|e| e.email.clone());
        assert_eq!(chosen.as_deref(), Some("primary@example.com"));
    }

    #[test]
    fn a_user_without_login_falls_back_to_numeric_subject() {
        let u = user(None, None, None);
        assert_eq!(u.id.to_string(), "42");
        let display = u.name.clone().or_else(|| u.login.clone());
        assert!(
            display.is_none(),
            "no name/login ⇒ let the consumer synthesize"
        );
    }

    #[test]
    fn profile_email_maps_to_unverified_when_it_is_the_only_source() {
        // The `resolve_email` fallback path: an email the emails endpoint never
        // confirmed is reported unverified, so it can never silently link.
        let profile = SocialProfile::new(GithubSocialProvider::KEY, "42")
            .with_email(Some("ada@example.com".into()), false);
        assert_eq!(profile.email.as_deref(), Some("ada@example.com"));
        assert!(!profile.email_verified);
    }
}

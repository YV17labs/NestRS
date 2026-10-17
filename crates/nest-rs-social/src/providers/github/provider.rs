use nest_rs_authn::AuthError;
use nest_rs_oauth_client::{OAuthClient, TokenSet};
use serde::Deserialize;

use super::config::GithubSocialConfig;
use super::module::GithubSocialModule;
use crate::provider::{ProfileFuture, SocialProfile, SocialProvider};
use crate::registry::{SocialProviderEntry, resolve_provider};

/// The GitHub social provider; overrides only [`SocialProvider::profile`].
pub struct GithubSocialProvider {
    client: OAuthClient,
}

impl GithubSocialProvider {
    pub(crate) const KEY: &'static str = "github";
    const EMAILS_URL: &'static str = "https://api.github.com/user/emails";

    pub(crate) fn new(client: OAuthClient) -> Self {
        Self { client }
    }

    /// GitHub's `GET /user` does not attest verification: prefer the primary
    /// verified address from the emails endpoint at `emails_url`, else the
    /// profile email, unverified.
    async fn resolve_email(
        &self,
        emails_url: &str,
        profile_email: Option<String>,
        access_token: &str,
    ) -> (Option<String>, bool) {
        match self
            .client
            .fetch::<Vec<GithubEmail>>(emails_url, access_token)
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
            // A documented default, so said at `warn`: only a verified email may
            // link a login to an existing account, and a returning user on the
            // fallback cannot reach theirs.
            Err(err) => {
                tracing::warn!(
                    target: crate::TARGET,
                    provider = Self::KEY,
                    error = %nest_rs_core::error_message(&err),
                    "github verified-email lookup failed; the profile email is used unverified",
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

    fn client(&self) -> &OAuthClient {
        &self.client
    }

    fn profile<'a>(&'a self, tokens: &'a TokenSet) -> ProfileFuture<'a> {
        Box::pin(async move {
            let user: GithubUser = self.client.userinfo(&tokens.access_token).await?;
            let display_name = user.name.or(user.login);
            let (email, verified) = self
                .resolve_email(Self::EMAILS_URL, user.email, &tokens.access_token)
                .await;
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
                let client = OAuthClient::new(config.oauth2_config()?)
                    .map_err(|e| anyhow::anyhow!("invalid GitHub OAuth2 client config: {e}"))?;
                Ok(GithubSocialProvider::new(client))
            })
        },
        module: || std::any::TypeId::of::<GithubSocialModule>(),
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

    /// GitHub's three sequential calls must fit inside `AuthnGuard`'s net, or it
    /// would replace the client's sentence naming the endpoint with its own.
    #[test]
    fn a_login_s_provider_calls_fit_inside_the_guard_s_net() {
        const CALLS: u32 = 3;
        let login = OAuthClient::CALL_TIMEOUT * CALLS;
        assert!(
            login < nest_rs_authn::AUTHENTICATE_TIMEOUT,
            "{login:?} vs {:?}",
            nest_rs_authn::AUTHENTICATE_TIMEOUT,
        );
    }

    /// A GitHub whose emails endpoint fails: the login falls back to the
    /// profile email, unverified, and says so at `warn` with the chain — a
    /// returning user on that fallback cannot reach their account.
    #[tokio::test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the double answers best-effort; the test asserts what the client made of it"
    )]
    async fn a_failed_verified_email_lookup_is_said_at_warn_and_falls_back_unverified() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let authority = nest_rs_testing::TestAuthority::new();
        let acceptor = authority.server(&["127.0.0.1"]).acceptor(None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let addr = listener.local_addr().expect("its address");
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    continue;
                };
                let mut head = Vec::new();
                let mut buf = [0_u8; 4096];
                while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                    match tls.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => head.extend_from_slice(&buf[..read]),
                    }
                }
                let _ = tls
                    .write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\
                          connection: close\r\n\r\n",
                    )
                    .await;
                let _ = tls.shutdown().await;
            }
        });
        let client = OAuthClient::new(nest_rs_oauth_client::OAuthClientConfig {
            userinfo_url: format!("https://{addr}/user"),
            tls: authority.client_tls(),
            ..GithubSocialConfig {
                client_id: "id".into(),
                client_secret: "secret".into(),
                redirect_url: "https://app.example/social/github/callback".into(),
                scopes: Vec::new(),
            }
            .oauth2_config()
            .expect("an https redirect composes")
        })
        .expect("the client builds");
        let provider = GithubSocialProvider::new(client);
        let logs = nest_rs_testing::LogCapture::install();

        let (email, verified) = provider
            .resolve_email(
                &format!("https://{addr}/user/emails"),
                Some("ada@example.com".into()),
                "gho_access-token",
            )
            .await;

        assert_eq!(email.as_deref(), Some("ada@example.com"));
        assert!(!verified, "the profile email is never taken as verified");
        let event = logs.expect_one(
            crate::TARGET,
            "github verified-email lookup failed; the profile email is used unverified",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("provider").as_deref(), Some("github"));
        let error = event.field("error").expect("the chain is filed");
        assert!(error.contains("answered 503"), "{error}");
        assert!(!error.contains("gho_access-token"), "{error}");
    }

    #[test]
    fn profile_email_maps_to_unverified_when_it_is_the_only_source() {
        let profile = SocialProfile::new(GithubSocialProvider::KEY, "42")
            .with_email(Some("ada@example.com".into()), false);
        assert_eq!(profile.email.as_deref(), Some("ada@example.com"));
        assert!(!profile.email_verified);
    }
}

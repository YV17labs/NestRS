use nest_rs_authn::OAuth2Config;
use nest_rs_config::{Config, ConfigService, config};
use validator::Validate;

use crate::registry::SocialProviderConfig;

/// GitHub OAuth deployment config. Only credentials, redirect, and scopes are
/// deployment config — the auth/token/userinfo endpoint URLs are provider
/// constants (see `GithubSocialConfig::oauth2_config`).
///
/// Dual-path (env `NESTRS_SOCIAL__GITHUB__*` **and** the pinned struct). No
/// `Debug`: `client_secret` must not leak through a derived format.
#[config(namespace = "social__github")]
#[derive(Clone, Default, Validate)]
pub struct GithubSocialConfig {
    /// The GitHub OAuth app's client id.
    #[validate(length(min = 1))]
    pub client_id: String,
    /// The GitHub OAuth app's client secret.
    #[validate(length(min = 1))]
    pub client_secret: String,
    /// The registered redirect URL the callback returns to.
    #[validate(length(min = 1))]
    pub redirect_url: String,
    /// Defaults to `read:user user:email` (the canonical login set) when unset
    /// — see `GithubSocialConfig::scopes_or_default`.
    pub scopes: Vec<String>,
}

impl GithubSocialConfig {
    const AUTH_URL: &'static str = "https://github.com/login/oauth/authorize";
    const TOKEN_URL: &'static str = "https://github.com/login/oauth/access_token";
    const USERINFO_URL: &'static str = "https://api.github.com/user";
    const DEFAULT_SCOPES: [&'static str; 2] = ["read:user", "user:email"];

    fn scopes_or_default(&self) -> Vec<String> {
        if self.scopes.is_empty() {
            Self::DEFAULT_SCOPES
                .iter()
                .map(|s| (*s).to_owned())
                .collect()
        } else {
            self.scopes.clone()
        }
    }

    /// Compose the transport-level [`OAuth2Config`] the shared client needs:
    /// deployment credentials + provider-constant endpoints.
    pub(crate) fn oauth2_config(&self) -> OAuth2Config {
        OAuth2Config {
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            auth_url: Self::AUTH_URL.to_owned(),
            token_url: Self::TOKEN_URL.to_owned(),
            redirect_url: self.redirect_url.clone(),
            userinfo_url: Self::USERINFO_URL.to_owned(),
            scopes: self.scopes_or_default(),
        }
    }
}

impl Config for GithubSocialConfig {
    fn from_env(env: &ConfigService) -> nest_rs_config::Result<Self> {
        Ok(Self {
            client_id: env.get("CLIENT_ID").unwrap_or_default(),
            client_secret: env.get("CLIENT_SECRET").unwrap_or_default(),
            redirect_url: env.get("REDIRECT_URL").unwrap_or_default(),
            scopes: env.list("SCOPES"),
        })
    }
}

impl SocialProviderConfig for GithubSocialConfig {
    /// `scopes` alone does not count as configuration — it has a default, so a
    /// deployment that sets only scopes still has no GitHub app to talk to.
    fn is_unconfigured(&self) -> bool {
        self.client_id.is_empty() && self.client_secret.is_empty() && self.redirect_url.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete() -> GithubSocialConfig {
        GithubSocialConfig {
            client_id: "id".into(),
            client_secret: "secret".into(),
            redirect_url: "https://app.example/social/github/callback".into(),
            scopes: vec![],
        }
    }

    #[test]
    fn scopes_default_to_the_canonical_login_set_when_unset() {
        assert_eq!(
            complete().scopes_or_default(),
            vec!["read:user".to_string(), "user:email".into()],
        );
    }

    #[test]
    fn explicit_scopes_win_over_the_default() {
        let cfg = GithubSocialConfig {
            scopes: vec!["read:org".into()],
            ..complete()
        };
        assert_eq!(cfg.scopes_or_default(), vec!["read:org".to_string()]);
    }

    #[test]
    fn oauth2_config_carries_provider_constant_endpoints() {
        let oauth = complete().oauth2_config();
        assert_eq!(oauth.auth_url, GithubSocialConfig::AUTH_URL);
        assert_eq!(oauth.token_url, GithubSocialConfig::TOKEN_URL);
        assert_eq!(oauth.userinfo_url, GithubSocialConfig::USERINFO_URL);
        assert_eq!(oauth.client_id, "id");
    }
}

use nest_rs_authn::OAuth2Config;
use nest_rs_config::{Config, ConfigService, config};
use validator::Validate;

use crate::registry::SocialProviderConfig;

/// Google OIDC deployment config. Dual-path (env `NESTRS_SOCIAL__GOOGLE__*`
/// **and** the pinned struct). No `Debug`: `client_secret` must not leak.
#[config(namespace = "social__google")]
#[derive(Clone, Default, Validate)]
pub struct GoogleSocialConfig {
    /// The Google OAuth client id.
    #[validate(length(min = 1))]
    pub client_id: String,
    /// The Google OAuth client secret.
    #[validate(length(min = 1))]
    pub client_secret: String,
    /// The registered redirect URL the callback returns to.
    #[validate(length(min = 1))]
    pub redirect_url: String,
    /// Defaults to `openid email profile` when unset.
    pub scopes: Vec<String>,
}

impl GoogleSocialConfig {
    const AUTH_URL: &'static str = "https://accounts.google.com/o/oauth2/v2/auth";
    const TOKEN_URL: &'static str = "https://oauth2.googleapis.com/token";
    const USERINFO_URL: &'static str = "https://openidconnect.googleapis.com/v1/userinfo";
    const DEFAULT_SCOPES: [&'static str; 3] = ["openid", "email", "profile"];

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

impl Config for GoogleSocialConfig {
    fn from_env(env: &ConfigService) -> nest_rs_config::Result<Self> {
        Ok(Self {
            client_id: env.get("CLIENT_ID").unwrap_or_default(),
            client_secret: env.get("CLIENT_SECRET").unwrap_or_default(),
            redirect_url: env.get("REDIRECT_URL").unwrap_or_default(),
            scopes: env.list("SCOPES"),
        })
    }
}

impl SocialProviderConfig for GoogleSocialConfig {
    /// `scopes` alone does not count as configuration — it has a default, so a
    /// deployment that sets only scopes still has no Google app to talk to.
    fn is_unconfigured(&self) -> bool {
        self.client_id.is_empty() && self.client_secret.is_empty() && self.redirect_url.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_default_to_the_oidc_login_set_when_unset() {
        let cfg = GoogleSocialConfig {
            client_id: "id".into(),
            client_secret: "secret".into(),
            redirect_url: "https://app.example/social/google/callback".into(),
            scopes: vec![],
        };
        assert_eq!(
            cfg.scopes_or_default(),
            vec!["openid".to_string(), "email".into(), "profile".into()],
        );
        assert_eq!(cfg.oauth2_config().auth_url, GoogleSocialConfig::AUTH_URL);
    }
}

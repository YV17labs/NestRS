use nest_rs_authn::AuthError;
use nest_rs_config::{ClientTls, Config, ConfigService, Namespaced, config, spellings};
use nest_rs_oauth_client::{OAuthClientConfig, refuse_plaintext_redirect};

use crate::registry::SocialProviderConfig;

/// Google OIDC deployment config. No `Debug`: `client_secret` must not leak.
#[config(namespace = "social__google")]
#[derive(Clone, Default)]
pub struct GoogleSocialConfig {
    /// The Google OAuth client id.
    #[validate(length(min = 1), custom(function = "crate::providers::not_blank"))]
    pub client_id: String,
    /// The Google OAuth client secret.
    #[validate(length(min = 1), custom(function = "crate::providers::not_blank"))]
    pub client_secret: String,
    /// The registered redirect URL the callback returns to.
    #[validate(length(min = 1), custom(function = "crate::providers::not_blank"))]
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

    /// Compose the transport-level [`OAuthClientConfig`] the shared client needs,
    /// the redirect refused under this config's own variable when the client
    /// would refuse it.
    pub(crate) fn oauth2_config(&self) -> Result<OAuthClientConfig, AuthError> {
        refuse_plaintext_redirect(
            &self.redirect_url,
            &format!(
                "{}, or `GoogleSocialConfig::redirect_url` in code",
                spellings(Self::NAMESPACE, "REDIRECT_URL")
            ),
        )?;
        Ok(OAuthClientConfig {
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            auth_url: Self::AUTH_URL.to_owned(),
            token_url: Self::TOKEN_URL.to_owned(),
            redirect_url: self.redirect_url.clone(),
            userinfo_url: Self::USERINFO_URL.to_owned(),
            scopes: self.scopes_or_default(),
            tls: ClientTls::default(),
        })
    }
}

impl Config for GoogleSocialConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            client_id: env.get("CLIENT_ID")?.unwrap_or(base.client_id),
            client_secret: env.get("CLIENT_SECRET")?.unwrap_or(base.client_secret),
            redirect_url: env.get("REDIRECT_URL")?.unwrap_or(base.redirect_url),
            scopes: env.list("SCOPES", base.scopes)?,
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

    fn complete() -> GoogleSocialConfig {
        GoogleSocialConfig {
            client_id: "id".into(),
            client_secret: "secret".into(),
            redirect_url: "https://app.example/social/google/callback".into(),
            scopes: vec![],
        }
    }

    #[test]
    fn scopes_default_to_the_oidc_login_set_when_unset() {
        let cfg = complete();
        assert_eq!(
            cfg.scopes_or_default(),
            vec!["openid".to_string(), "email".into(), "profile".into()],
        );
        let oauth = cfg.oauth2_config().expect("an https redirect composes");
        assert_eq!(oauth.auth_url, GoogleSocialConfig::AUTH_URL);
    }

    /// The deployment set `<PREFIX>_SOCIAL__GOOGLE__REDIRECT_URL`, so a refused
    /// redirect names that variable, never the OAuth client's, nor the URL.
    #[test]
    fn a_refused_redirect_names_the_providers_own_variable() {
        for (redirect, why) in [
            (
                "http://u:s3cr3t@app.example/social/google/callback",
                "RFC 8252 §7.3",
            ),
            ("http://u:s3cr3t@[::1", "does not parse as a URL"),
        ] {
            let Err(sentence @ AuthError::Failed { .. }) = GoogleSocialConfig {
                redirect_url: redirect.into(),
                ..complete()
            }
            .oauth2_config() else {
                panic!("{redirect} is refused");
            };
            let sentence = nest_rs_core::error_message(&sentence);
            assert!(
                sentence.contains(&nest_rs_config::var_name("social__google", "REDIRECT_URL"))
                    && sentence.contains("`GoogleSocialConfig::redirect_url` in code")
                    && sentence.contains(why),
                "{sentence}"
            );
            assert!(
                !sentence.contains("OAUTH__CLIENT") && !sentence.contains("s3cr3t"),
                "{sentence}"
            );
        }
    }

    #[test]
    fn a_loopback_redirect_composes() {
        let config = GoogleSocialConfig {
            redirect_url: "http://127.0.0.1:3000/social/google/callback".into(),
            ..complete()
        };
        assert!(config.oauth2_config().is_ok());
    }

    #[test]
    fn whitespace_only_credentials_are_refused_like_a_partial_set() {
        use validator::Validate;

        type Setter = fn(&mut GoogleSocialConfig);
        let setters: [(Setter, &str); 3] = [
            (|c| c.client_id = "  ".into(), "client_id"),
            (|c| c.client_secret = "\t".into(), "client_secret"),
            (|c| c.redirect_url = "\n".into(), "redirect_url"),
        ];
        for (mutate, field) in setters {
            let mut cfg = complete();
            mutate(&mut cfg);
            assert!(!cfg.is_unconfigured(), "{field}: blank is not unconfigured");
            let err = cfg.validate().expect_err("blank is refused");
            assert!(err.field_errors().contains_key(field), "{field}: {err:?}");
        }
    }
}

use nest_rs_authn::AuthError;
use nest_rs_config::{ClientTls, Namespaced, config, endpoint_shown};
use nest_rs_core::Secret;
use nest_rs_oauth_client::{AuthorizationParameters, OAuthClientConfig, refuse_plaintext_redirect};

use crate::registry::SocialProviderConfig;

/// Google OIDC deployment config.
///
/// `Debug` shows `redirect_url` through [`endpoint_shown`]: its userinfo or
/// query may carry a credential.
#[config(namespace = "social__google")]
#[derive(Clone, Default)]
pub struct GoogleSocialConfig {
    /// The Google OAuth client id.
    #[validate(length(min = 1), custom(function = "crate::providers::not_blank"))]
    pub client_id: String,
    /// The Google OAuth client secret.
    #[validate(
        length(min = 1),
        custom(function = "crate::providers::not_blank_secret")
    )]
    pub client_secret: Secret<String>,
    /// The registered redirect URL the callback returns to.
    #[validate(length(min = 1), custom(function = "crate::providers::not_blank"))]
    pub redirect_url: String,
    /// Defaults to `openid email profile` when unset; each an RFC 6749 §3.3
    /// `scope-token`, so one holding a space or a `"` fails the boot.
    #[validate(custom(function = "OAuthClientConfig::validate_scopes"))]
    pub scopes: Vec<String>,
    /// What the authorization request carries beside the client's own
    /// parameters (key `AUTHORIZATION_PARAMETERS`, a JSON object of strings);
    /// none by default. A parameter the client sends itself fails the boot
    /// ([`AuthorizationParameters`]).
    #[config(json)]
    #[validate(custom(function = "OAuthClientConfig::validate_authorization_parameters"))]
    pub authorization_parameters: AuthorizationParameters,
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
            &Self::REDIRECT_URL.spellings_or_field(Self::NAMESPACE),
        )?;
        Ok(OAuthClientConfig {
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            auth_url: Self::AUTH_URL.to_owned(),
            token_url: Self::TOKEN_URL.to_owned(),
            redirect_url: self.redirect_url.clone(),
            userinfo_url: Self::USERINFO_URL.to_owned(),
            scopes: self.scopes_or_default(),
            authorization_parameters: self.authorization_parameters.clone(),
            tls: ClientTls::default(),
        })
    }
}

impl std::fmt::Debug for GoogleSocialConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            client_id,
            client_secret,
            redirect_url,
            scopes,
            authorization_parameters,
        } = self;
        f.debug_struct("GoogleSocialConfig")
            .field("client_id", client_id)
            .field("client_secret", client_secret)
            .field("redirect_url", &endpoint_shown(redirect_url))
            .field("scopes", scopes)
            .field("authorization_parameters", authorization_parameters)
            .finish()
    }
}

impl SocialProviderConfig for GoogleSocialConfig {
    /// `scopes` and `authorization_parameters` alone do not count as
    /// configuration — neither names an app, so a deployment that sets only
    /// them still has no Google app to talk to.
    fn is_unconfigured(&self) -> bool {
        self.client_id.is_empty()
            && self.client_secret.expose_secret().is_empty()
            && self.redirect_url.is_empty()
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
            authorization_parameters: Default::default(),
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

    /// The process declares the test profile, as every harness does: there,
    /// and in development, a loopback redirect composes.
    #[test]
    fn a_loopback_redirect_composes_outside_a_deployment() {
        nest_rs_testing::load_project_env();
        let config = GoogleSocialConfig {
            redirect_url: "http://127.0.0.1:3000/social/google/callback".into(),
            ..complete()
        };
        assert!(config.oauth2_config().is_ok());
    }

    /// A deployment writes the scopes as a list, `SCOPES=a,b`: one entry holding
    /// a space or a `"` is no scope RFC 6749 §3.3 can carry, refused at boot
    /// naming the provider's own variable, never the scope.
    #[test]
    fn a_scope_outside_the_token_grammar_is_refused_naming_the_providers_variable() {
        use nest_rs_config::{Config, ConfigService};
        use validator::Validate;

        for scopes in ["openid email", "openid,never\"shown"] {
            let env = ConfigService::with_vars("social__google", [("SCOPES", scopes)]);
            let config = GoogleSocialConfig::from_env(&env, complete()).expect("the list reads");
            let err = config.validate().expect_err("refused");
            let rendered =
                nest_rs_config::ConfigError::validation(GoogleSocialConfig::NAMESPACE, err)
                    .to_string();
            assert!(
                rendered.contains(&format!(
                    "scopes ({}): not_a_scope_token",
                    nest_rs_config::var_name("social__google", "SCOPES")
                )),
                "{rendered}"
            );
            assert!(!rendered.contains("never"), "{rendered}");
        }
    }

    /// The parameters a deployment sets under the provider's own variable reach
    /// the provider's authorization request beside the client's own.
    #[test]
    fn the_authorization_parameters_set_reach_the_providers_request() {
        use nest_rs_config::{Config, ConfigService};

        let env = ConfigService::with_vars(
            "social__google",
            [(
                "AUTHORIZATION_PARAMETERS",
                r#"{"access_type":"offline","prompt":"consent"}"#,
            )],
        );
        let config = GoogleSocialConfig::from_env(&env, complete()).expect("the variable reads");
        let jwt = nest_rs_authn::JwtService::new(nest_rs_authn::JwtOptions::new(
            "a-secret-padded-to-thirty-two-bytes",
        ))
        .expect("a signer");

        let client =
            nest_rs_oauth_client::OAuthClient::new(config.oauth2_config().expect("composes"))
                .expect("builds");
        let redirect = client.authorize(&jwt, "google", None).expect("authorize");

        let pairs: Vec<&str> = redirect.url().split(['?', '&']).skip(1).collect();
        for sent in ["access_type=offline", "prompt=consent"] {
            assert!(pairs.contains(&sent), "{sent}: {pairs:?}");
        }
    }

    /// A parameter the client sends itself is refused under the provider's own
    /// variable.
    #[test]
    fn a_parameter_the_client_sends_itself_is_refused_naming_the_providers_variable() {
        use nest_rs_config::{Config, ConfigService};
        use validator::Validate;

        let env = ConfigService::with_vars(
            "social__google",
            [(
                "AUTHORIZATION_PARAMETERS",
                r#"{"redirect_uri":"https://never.example"}"#,
            )],
        );
        let config = GoogleSocialConfig::from_env(&env, complete()).expect("the variable reads");
        let rendered = nest_rs_config::ConfigError::validation(
            GoogleSocialConfig::NAMESPACE,
            config.validate().expect_err("refused"),
        )
        .to_string();
        assert!(
            rendered.contains(&format!(
                "authorization_parameters ({}): sent_by_the_client",
                nest_rs_config::var_name("social__google", "AUTHORIZATION_PARAMETERS")
            )),
            "{rendered}"
        );
        assert!(!rendered.contains("never"), "{rendered}");
    }

    #[test]
    fn debug_never_prints_the_client_secret() {
        let printed = format!("{:?}", complete());
        assert!(!printed.contains("\"secret\""), "{printed}");
        assert!(printed.contains("client_secret: <redacted>"), "{printed}");
        assert!(printed.contains("\"id\""), "the client id stays: {printed}");
    }

    #[test]
    fn debug_shows_the_redirect_without_its_userinfo_or_query() {
        let printed = format!(
            "{:?}",
            GoogleSocialConfig {
                redirect_url: "https://u:URLSECRET1@app.example/callback?k=URLSECRET2".into(),
                ..complete()
            }
        );
        assert!(!printed.contains("URLSECRET"), "{printed}");
        assert!(
            printed.contains("redirect_url: \"https://app.example/callback\""),
            "{printed}"
        );
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

use nest_rs_authn::AuthError;
use nest_rs_config::{ClientTls, Namespaced, config, endpoint_shown};
use nest_rs_core::Secret;
use nest_rs_oauth_client::{AuthorizationParameters, OAuthClientConfig, refuse_plaintext_redirect};

use crate::registry::SocialProviderConfig;

/// GitHub OAuth deployment config; the endpoint URLs are provider constants.
///
/// `Debug` shows `redirect_url` through [`endpoint_shown`]: its userinfo or
/// query may carry a credential.
#[config(namespace = "social__github")]
#[derive(Clone, Default)]
pub struct GithubSocialConfig {
    /// The GitHub OAuth app's client id.
    #[validate(length(min = 1), custom(function = "crate::providers::not_blank"))]
    pub client_id: String,
    /// The GitHub OAuth app's client secret.
    #[validate(
        length(min = 1),
        custom(function = "crate::providers::not_blank_secret")
    )]
    pub client_secret: Secret<String>,
    /// The registered redirect URL the callback returns to.
    #[validate(length(min = 1), custom(function = "crate::providers::not_blank"))]
    pub redirect_url: String,
    /// Defaults to `read:user user:email` when unset; each an RFC 6749 §3.3
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

    /// Compose the transport-level [`OAuthClientConfig`] the shared client needs:
    /// deployment credentials + provider-constant endpoints, the redirect refused
    /// under this config's own variable when the client would refuse it.
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

impl std::fmt::Debug for GithubSocialConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            client_id,
            client_secret,
            redirect_url,
            scopes,
            authorization_parameters,
        } = self;
        f.debug_struct("GithubSocialConfig")
            .field("client_id", client_id)
            .field("client_secret", client_secret)
            .field("redirect_url", &endpoint_shown(redirect_url))
            .field("scopes", scopes)
            .field("authorization_parameters", authorization_parameters)
            .finish()
    }
}

impl SocialProviderConfig for GithubSocialConfig {
    /// `scopes` and `authorization_parameters` alone do not count as
    /// configuration — neither names an app, so a deployment that sets only
    /// them still has no GitHub app to talk to.
    fn is_unconfigured(&self) -> bool {
        self.client_id.is_empty()
            && self.client_secret.expose_secret().is_empty()
            && self.redirect_url.is_empty()
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
            authorization_parameters: Default::default(),
        }
    }

    /// A deployment writes the scopes as a list, `SCOPES=a,b`: one entry holding
    /// a space or a `"` is no scope RFC 6749 §3.3 can carry, refused at boot
    /// naming the provider's own variable, never the scope.
    #[test]
    fn a_scope_outside_the_token_grammar_is_refused_naming_the_providers_variable() {
        use nest_rs_config::{Config, ConfigService};
        use validator::Validate;

        for scopes in ["read:user user:email", "openid,never\"shown"] {
            let env = ConfigService::with_vars("social__github", [("SCOPES", scopes)]);
            let config = GithubSocialConfig::from_env(&env, complete()).expect("the list reads");
            let err = config.validate().expect_err("refused");
            let rendered =
                nest_rs_config::ConfigError::validation(GithubSocialConfig::NAMESPACE, err)
                    .to_string();
            assert!(
                rendered.contains(&format!(
                    "scopes ({}): not_a_scope_token",
                    nest_rs_config::var_name("social__github", "SCOPES")
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
            "social__github",
            [(
                "AUTHORIZATION_PARAMETERS",
                r#"{"prompt":"select_account","allow_signup":"false"}"#,
            )],
        );
        let config = GithubSocialConfig::from_env(&env, complete()).expect("the variable reads");
        let mut options = nest_rs_authn::JwtOptions::new("a-secret-padded-to-thirty-two-bytes");
        options.issuer = Some("https://app.example".into());
        options.audience = Some("https://app.example".into());
        let jwt = nest_rs_authn::JwtService::new(options).expect("a signer");

        let client =
            nest_rs_oauth_client::OAuthClient::new(config.oauth2_config().expect("composes"))
                .expect("builds");
        let redirect = client.authorize(&jwt, "github", None).expect("authorize");

        let pairs: Vec<&str> = redirect.url().split(['?', '&']).skip(1).collect();
        for sent in ["prompt=select_account", "allow_signup=false"] {
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
            "social__github",
            [(
                "AUTHORIZATION_PARAMETERS",
                r#"{"redirect_uri":"https://never.example"}"#,
            )],
        );
        let config = GithubSocialConfig::from_env(&env, complete()).expect("the variable reads");
        let rendered = nest_rs_config::ConfigError::validation(
            GithubSocialConfig::NAMESPACE,
            config.validate().expect_err("refused"),
        )
        .to_string();
        assert!(
            rendered.contains(&format!(
                "authorization_parameters ({}): sent_by_the_client",
                nest_rs_config::var_name("social__github", "AUTHORIZATION_PARAMETERS")
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
            GithubSocialConfig {
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

        type Setter = fn(&mut GithubSocialConfig);
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
        let oauth = complete()
            .oauth2_config()
            .expect("an https redirect composes");
        assert_eq!(oauth.auth_url, GithubSocialConfig::AUTH_URL);
        assert_eq!(oauth.token_url, GithubSocialConfig::TOKEN_URL);
        assert_eq!(oauth.userinfo_url, GithubSocialConfig::USERINFO_URL);
        assert_eq!(oauth.client_id, "id");
    }

    /// The deployment set `<PREFIX>_SOCIAL__GITHUB__REDIRECT_URL`, so a refused
    /// redirect names that variable, never the OAuth client's, nor the URL.
    #[test]
    fn a_refused_redirect_names_the_providers_own_variable() {
        for (redirect, why) in [
            (
                "http://u:s3cr3t@app.example/social/github/callback",
                "RFC 8252 §7.3",
            ),
            ("http://u:s3cr3t@[::1", "does not parse as a URL"),
        ] {
            let Err(sentence @ AuthError::Failed { .. }) = GithubSocialConfig {
                redirect_url: redirect.into(),
                ..complete()
            }
            .oauth2_config() else {
                panic!("{redirect} is refused");
            };
            let sentence = nest_rs_core::error_message(&sentence);
            assert!(
                sentence.contains(&nest_rs_config::var_name("social__github", "REDIRECT_URL"))
                    && sentence.contains("`GithubSocialConfig::redirect_url` in code")
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
        let config = GithubSocialConfig {
            redirect_url: "http://localhost:3000/social/github/callback".into(),
            ..complete()
        };
        assert!(config.oauth2_config().is_ok());
    }
}

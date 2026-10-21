//! [`OAuthClientConfig`] — env-driven OAuth2 provider endpoints.

use nest_rs_authn::AuthError;
use nest_rs_config::{ClientTls, Config, ConfigService, Environment, config};
use oauth2::url::{Host, Url};

/// Env-driven OAuth2 provider endpoints (namespace `oauth__client`); every URL and
/// credential is required. No `Debug`: `client_secret` must not leak through a format.
///
/// The provider's endpoints are `https` URLs, and the redirect an `https` URL,
/// or `http` on a loopback address in a declared development or test profile:
/// [`OAuthClient::new`](crate::OAuthClient::new) refuses any other, naming the
/// setting and the RFC that requires TLS there.
#[config(namespace = "oauth__client")]
#[derive(Clone, Default)]
pub struct OAuthClientConfig {
    /// The registered OAuth2 client id.
    #[validate(length(min = 1), custom(function = "not_blank"))]
    pub client_id: String,
    /// The registered OAuth2 client secret.
    #[validate(length(min = 1), custom(function = "not_blank"))]
    pub client_secret: String,
    /// Provider authorization endpoint (where the user is redirected to consent).
    #[validate(length(min = 1), custom(function = "not_blank"))]
    pub auth_url: String,
    /// Provider token endpoint (where the auth code is exchanged for tokens).
    #[validate(length(min = 1), custom(function = "not_blank"))]
    pub token_url: String,
    /// This app's callback URL the provider redirects back to.
    #[validate(length(min = 1), custom(function = "not_blank"))]
    pub redirect_url: String,
    /// Provider userinfo endpoint used to fetch the caller's profile.
    #[validate(length(min = 1), custom(function = "not_blank"))]
    pub userinfo_url: String,
    /// Scopes requested at authorization; empty by default.
    pub scopes: Vec<String>,
    /// What the provider's certificate must chain to — the system's authorities
    /// unless `<PREFIX>_OAUTH__CLIENT__TLS_CA_CERT` names others, which replace
    /// them — and the certificate presented to a provider requiring one
    /// (`TLS_CERT` and `TLS_KEY`). The certificate is verified either way.
    pub tls: ClientTls,
}

/// A required value holding only whitespace is refused like an empty one.
fn not_blank(value: &str) -> Result<(), validator::ValidationError> {
    if value.trim().is_empty() {
        return Err(validator::ValidationError::new("blank"));
    }
    Ok(())
}

impl Config for OAuthClientConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            client_id: env.get("CLIENT_ID")?.unwrap_or(base.client_id),
            client_secret: env.get("CLIENT_SECRET")?.unwrap_or(base.client_secret),
            auth_url: env.get("AUTH_URL")?.unwrap_or(base.auth_url),
            token_url: env.get("TOKEN_URL")?.unwrap_or(base.token_url),
            redirect_url: env.get("REDIRECT_URL")?.unwrap_or(base.redirect_url),
            userinfo_url: env.get("USERINFO_URL")?.unwrap_or(base.userinfo_url),
            scopes: env.list("SCOPES", base.scopes)?,
            tls: ClientTls::from_env(env, base.tls)?,
        })
    }
}

impl OAuthClientConfig {
    /// Refuse a URL that does not parse, a provider endpoint that is not
    /// `https`, and a redirect that is neither `https` nor, in a profile that
    /// is no deployment, `http` on a loopback address — naming the setting,
    /// never the URL, which may carry userinfo. A setting is named by the
    /// `variables` the config was read under, beside its field, or by its
    /// field alone for a config its caller composed.
    pub(crate) fn refuse_plaintext(
        &self,
        variables: Option<&ConfigService>,
        environment: Environment,
    ) -> Result<(), AuthError> {
        let endpoints = [
            (
                &self.auth_url,
                "AUTH_URL",
                "auth_url",
                "RFC 6749 §3.1 requires TLS at the authorization endpoint, where the user signs \
                 in",
            ),
            (
                &self.token_url,
                "TOKEN_URL",
                "token_url",
                "RFC 6749 §3.2 requires TLS at the token endpoint, which receives the client \
                 secret and the authorization code",
            ),
            (
                &self.userinfo_url,
                "USERINFO_URL",
                "userinfo_url",
                "RFC 6750 §5.3 sends a bearer token over TLS alone, and the userinfo endpoint \
                 receives one",
            ),
        ];
        for (raw, key, field, why) in endpoints {
            let setting = setting(variables, key, field);
            if parsed(raw, &setting)?.scheme() != "https" {
                return Err(refused(&setting, &format!("must be an https URL — {why}")));
            }
        }
        refuse_plaintext_redirect_in(
            &self.redirect_url,
            &setting(variables, "REDIRECT_URL", "redirect_url"),
            environment,
        )
    }
}

/// Refuse a redirect URL that does not parse, or that is not `https` —
/// plain `http` is accepted on a loopback address alone (RFC 8252 §7.3), and
/// only in a declared development or test profile: the process's
/// `<PREFIX>_ENV` decides. The sentence names the redirect as `setting` and
/// never quotes it.
///
/// [`OAuthClient::new`](crate::OAuthClient::new) refuses
/// [`OAuthClientConfig::redirect_url`] this way under its own variable; a
/// provider that builds the config from a setting of its own refuses that
/// setting first, so a deployment reads the variable it set.
///
/// ```
/// use nest_rs_config::spellings;
/// use nest_rs_oauth_client::refuse_plaintext_redirect;
///
/// let setting = format!(
///     "{}, or `AcmeSocialConfig::redirect_url` in code",
///     spellings("social__acme", "REDIRECT_URL"),
/// );
/// assert!(refuse_plaintext_redirect("https://app.example/callback", &setting).is_ok());
/// let refused = refuse_plaintext_redirect("http://app.example/callback", &setting)
///     .expect_err("the authorization code would return in clear");
/// assert!(
///     nest_rs_core::error_message(&refused).contains(&format!("{setting}, must be an https URL"))
/// );
/// ```
pub fn refuse_plaintext_redirect(redirect_url: &str, setting: &str) -> Result<(), AuthError> {
    refuse_plaintext_redirect_in(redirect_url, setting, Environment::from_env())
}

/// [`refuse_plaintext_redirect`] under `environment`, the profile that decides
/// whether plain `http` on loopback is accepted.
fn refuse_plaintext_redirect_in(
    redirect_url: &str,
    setting: &str,
    environment: Environment,
) -> Result<(), AuthError> {
    let redirect = parsed(redirect_url, setting)?;
    let local = redirect.scheme() == "http" && is_loopback(&redirect) && !environment.is_deployed();
    if redirect.scheme() != "https" && !local {
        return Err(refused(
            setting,
            &format!(
                "must be an https URL — the authorization code returns there (RFC 6749 \
                 §3.1.2.1); plain http is accepted only on a loopback address (localhost, \
                 127.0.0.1, [::1]), which RFC 8252 §7.3 allows, in a declared development or \
                 test profile ({}, here {})",
                Environment::var_name(),
                environment.as_str(),
            ),
        ));
    }
    Ok(())
}

/// How a refusal names `field`: both ways it is set when the config was read
/// under `variables`, by the field alone when its caller composed it.
fn setting(variables: Option<&ConfigService>, key: &str, field: &str) -> String {
    match variables {
        Some(variables) => format!(
            "{}, or `OAuthClientConfig::{field}` in code",
            variables.spellings(key)
        ),
        None => format!("`OAuthClientConfig::{field}`"),
    }
}

/// `raw` parsed, refused naming `setting` when it does not parse.
fn parsed(raw: &str, setting: &str) -> Result<Url, AuthError> {
    Url::parse(raw.trim()).map_err(|error| {
        AuthError::failed_because(format!("{setting}, does not parse as a URL"), error)
    })
}

/// The refusal of `setting`, saying `why`.
fn refused(setting: &str, why: &str) -> AuthError {
    AuthError::failed(format!("{setting}, {why}"))
}

/// Whether `url` names this host's loopback interface.
fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_config::Namespaced;
    use validator::Validate;

    fn complete() -> OAuthClientConfig {
        OAuthClientConfig {
            client_id: "id".into(),
            client_secret: "secret".into(),
            auth_url: "https://auth.example/oauth/authorize".into(),
            token_url: "https://auth.example/oauth/token".into(),
            redirect_url: "https://app.example/oauth/callback".into(),
            userinfo_url: "https://auth.example/oauth/userinfo".into(),
            scopes: vec!["read".into()],
            tls: ClientTls::default(),
        }
    }

    /// The variables a module-built client's config was read under.
    fn variables() -> ConfigService {
        ConfigService::for_namespace(OAuthClientConfig::NAMESPACE)
    }

    /// Every profile, the development conveniences' and the deployments'.
    const PROFILES: [Environment; 4] = [
        Environment::Development,
        Environment::Test,
        Environment::Staging,
        Environment::Production,
    ];

    fn refusal(config: &OAuthClientConfig) -> String {
        let Err(refused @ AuthError::Failed { .. }) =
            config.refuse_plaintext(Some(&variables()), Environment::Development)
        else {
            panic!("the config is refused");
        };
        nest_rs_core::error_message(&refused)
    }

    /// A provider endpoint over plain http sends a secret, a code or a token in
    /// clear: refused naming its variable, the URL never quoted.
    #[test]
    fn a_provider_endpoint_over_plain_http_is_refused_naming_its_variable() {
        type Setter = fn(&mut OAuthClientConfig);
        let setters: [(Setter, &str, &str); 3] = [
            (
                |c| c.auth_url = "http://u:s3cr3t@auth.example/authorize".into(),
                "AUTH_URL",
                "RFC 6749 §3.1",
            ),
            (
                |c| c.token_url = "http://u:s3cr3t@auth.example/token".into(),
                "TOKEN_URL",
                "RFC 6749 §3.2",
            ),
            (
                |c| c.userinfo_url = "http://u:s3cr3t@auth.example/userinfo".into(),
                "USERINFO_URL",
                "RFC 6750 §5.3",
            ),
        ];
        for (mutate, key, rfc) in setters {
            let mut config = complete();
            mutate(&mut config);
            let sentence = refusal(&config);
            assert!(
                sentence.contains(&nest_rs_config::var_name("oauth__client", key))
                    && sentence.contains("must be an https URL")
                    && sentence.contains(rfc),
                "{sentence}"
            );
            assert!(!sentence.contains("s3cr3t"), "{sentence}");
        }
    }

    const LOOPBACK: [&str; 3] = [
        "http://localhost:3000/callback",
        "http://127.0.0.1:3000/callback",
        "http://[::1]:3000/callback",
    ];

    /// A development or test profile may take the code back over loopback
    /// http, on every spelling of it; an https redirect is taken everywhere.
    #[test]
    fn a_loopback_redirect_is_accepted_on_every_spelling_outside_a_deployment() {
        for environment in [Environment::Development, Environment::Test] {
            for redirect in LOOPBACK {
                let config = OAuthClientConfig {
                    redirect_url: redirect.into(),
                    ..complete()
                };
                assert!(
                    config
                        .refuse_plaintext(Some(&variables()), environment)
                        .is_ok(),
                    "{environment:?}: {redirect}"
                );
            }
        }
        for environment in PROFILES {
            assert!(
                complete()
                    .refuse_plaintext(Some(&variables()), environment)
                    .is_ok(),
                "{environment:?}"
            );
        }
    }

    /// A staging or production profile — an undeclared one included — takes
    /// the code back over TLS alone, and the refusal names the profile and the
    /// variable that declares it.
    #[test]
    fn a_loopback_redirect_is_refused_in_a_deployment_naming_the_profile() {
        for environment in [Environment::Staging, Environment::Production] {
            for redirect in LOOPBACK {
                let Err(refused) = OAuthClientConfig {
                    redirect_url: redirect.into(),
                    ..complete()
                }
                .refuse_plaintext(Some(&variables()), environment) else {
                    panic!("{environment:?}: {redirect} is refused");
                };
                let sentence = nest_rs_core::error_message(&refused);
                assert!(
                    sentence.contains(&nest_rs_config::var_name("oauth__client", "REDIRECT_URL"))
                        && sentence.contains(&format!(
                            "({}, here {})",
                            Environment::var_name(),
                            environment.as_str()
                        )),
                    "{sentence}"
                );
            }
        }
    }

    /// The public refusal reads the process's profile: an undeclared one is
    /// production, which refuses loopback http.
    #[test]
    fn the_public_refusal_reads_the_profile_the_process_declares() {
        let declared = Environment::from_env();
        assert_eq!(
            refuse_plaintext_redirect(LOOPBACK[0], "REDIRECT").is_ok(),
            !declared.is_deployed(),
            "{declared:?}"
        );
    }

    /// A config its caller composed — a social provider's — is read under
    /// none of this crate's variables: a refusal names the field alone.
    #[test]
    fn a_composed_config_is_refused_by_its_field_alone() {
        let Err(refused) = OAuthClientConfig {
            token_url: "http://auth.example/oauth/token".into(),
            ..complete()
        }
        .refuse_plaintext(None, Environment::Production) else {
            panic!("a plaintext token endpoint is refused");
        };
        let sentence = nest_rs_core::error_message(&refused);
        assert!(
            sentence.contains("`OAuthClientConfig::token_url`, must be an https URL")
                && !sentence.contains(&nest_rs_config::var_name("oauth__client", "")),
            "{sentence}"
        );
    }

    #[test]
    fn a_redirect_over_plain_http_elsewhere_is_refused() {
        for redirect in [
            "http://app.example/callback",
            "http://localhost.app.example/callback",
            "http://10.0.0.1/callback",
        ] {
            let sentence = refusal(&OAuthClientConfig {
                redirect_url: redirect.into(),
                ..complete()
            });
            assert!(
                sentence.contains(&nest_rs_config::var_name("oauth__client", "REDIRECT_URL"))
                    && sentence.contains("RFC 8252 §7.3"),
                "{sentence}"
            );
            assert!(!sentence.contains(redirect), "{sentence}");
        }
    }

    #[test]
    fn a_url_that_does_not_parse_is_refused_without_its_value() {
        let sentence = refusal(&OAuthClientConfig {
            userinfo_url: "https://u:s3cr3t@[::1".into(),
            ..complete()
        });
        assert!(
            sentence.contains(&nest_rs_config::var_name("oauth__client", "USERINFO_URL"))
                && sentence.contains("does not parse as a URL"),
            "{sentence}"
        );
        assert!(!sentence.contains("s3cr3t"), "{sentence}");
    }

    #[test]
    fn default_is_invalid_until_every_url_is_filled() {
        let err = OAuthClientConfig::default().validate().unwrap_err();
        let fields = err.field_errors();
        for required in [
            "client_id",
            "client_secret",
            "auth_url",
            "token_url",
            "redirect_url",
            "userinfo_url",
        ] {
            assert!(
                fields.contains_key(required),
                "expected {required} in {:?}",
                fields.keys().collect::<Vec<_>>(),
            );
        }
    }

    #[test]
    fn a_complete_config_validates() {
        complete().validate().expect("valid");
        complete()
            .refuse_plaintext(Some(&variables()), Environment::Production)
            .expect("every URL is https");
    }

    #[test]
    fn each_required_field_is_individually_load_bearing() {
        type Setter = fn(&mut OAuthClientConfig);
        let setters: [(Setter, &str); 6] = [
            (|c| c.client_id = String::new(), "client_id"),
            (|c| c.client_secret = String::new(), "client_secret"),
            (|c| c.auth_url = String::new(), "auth_url"),
            (|c| c.token_url = String::new(), "token_url"),
            (|c| c.redirect_url = String::new(), "redirect_url"),
            (|c| c.userinfo_url = String::new(), "userinfo_url"),
        ];
        for (mutate, field) in setters {
            let mut cfg = complete();
            mutate(&mut cfg);
            let err = cfg.validate().unwrap_err();
            assert!(
                err.field_errors().contains_key(field),
                "blanking {field} did not trip validation: {err:?}",
            );
        }
    }

    #[test]
    fn a_whitespace_only_required_field_is_refused_like_an_empty_one() {
        type Setter = fn(&mut OAuthClientConfig);
        let setters: [(Setter, &str); 6] = [
            (|c| c.client_id = "  ".into(), "client_id"),
            (|c| c.client_secret = "\t".into(), "client_secret"),
            (|c| c.auth_url = " ".into(), "auth_url"),
            (|c| c.token_url = " ".into(), "token_url"),
            (|c| c.redirect_url = "\n".into(), "redirect_url"),
            (|c| c.userinfo_url = " ".into(), "userinfo_url"),
        ];
        for (mutate, field) in setters {
            let mut cfg = complete();
            mutate(&mut cfg);
            let err = cfg.validate().expect_err("blank is refused");
            assert!(
                err.field_errors().contains_key(field),
                "a blank {field} did not trip validation: {err:?}",
            );
        }
    }

    #[test]
    fn scopes_default_to_empty_and_take_an_explicit_list() {
        assert!(OAuthClientConfig::default().scopes.is_empty());
        let cfg = OAuthClientConfig {
            scopes: vec!["read".into(), "write".into()],
            ..complete()
        };
        assert_eq!(cfg.scopes, vec!["read".to_string(), "write".into()]);
    }
}

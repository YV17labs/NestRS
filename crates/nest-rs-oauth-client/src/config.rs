//! [`OAuthClientConfig`] — env-driven OAuth2 provider endpoints.

use nest_rs_authn::AuthError;
use nest_rs_config::{ClientTls, Config, ConfigService, Namespaced, config, spellings};
use oauth2::url::{Host, Url};

/// Env-driven OAuth2 provider endpoints (namespace `oauth__client`); every URL and
/// credential is required. No `Debug`: `client_secret` must not leak through a format.
///
/// The provider's endpoints are `https` URLs, and the redirect an `https` URL or
/// `http` on a loopback address: [`OAuthClient::new`](crate::OAuthClient::new)
/// refuses any other, naming the setting and the RFC that requires TLS there.
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
    /// `https`, and a redirect that is neither `https` nor `http` on a loopback
    /// address — naming the setting, never the URL, which may carry userinfo.
    pub(crate) fn refuse_plaintext(&self) -> Result<(), AuthError> {
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
            let setting = setting(key, field);
            if parsed(raw, &setting)?.scheme() != "https" {
                return Err(refused(&setting, &format!("must be an https URL — {why}")));
            }
        }
        refuse_plaintext_redirect(&self.redirect_url, &setting("REDIRECT_URL", "redirect_url"))
    }
}

/// Refuse a redirect URL that does not parse, or that is neither `https` nor
/// `http` on a loopback address (RFC 8252 §7.3), the sentence naming it as
/// `setting` and never quoting it.
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
/// assert!(refuse_plaintext_redirect("http://localhost:3000/callback", &setting).is_ok());
/// let refused = refuse_plaintext_redirect("http://app.example/callback", &setting)
///     .expect_err("the authorization code would return in clear");
/// assert!(refused.to_string().contains(&format!("{setting}, must be an https URL")));
/// ```
pub fn refuse_plaintext_redirect(redirect_url: &str, setting: &str) -> Result<(), AuthError> {
    let redirect = parsed(redirect_url, setting)?;
    let loopback = redirect.scheme() == "http" && is_loopback(&redirect);
    if redirect.scheme() != "https" && !loopback {
        return Err(refused(
            setting,
            "must be an https URL, or http on a loopback address (localhost, 127.0.0.1, \
             [::1]) — the authorization code returns there (RFC 6749 §3.1.2.1), and RFC 8252 \
             §7.3 allows plain http on loopback alone",
        ));
    }
    Ok(())
}

/// How a refusal names `field`: both ways it is set.
fn setting(key: &str, field: &str) -> String {
    format!(
        "{}, or `OAuthClientConfig::{field}` in code",
        spellings(OAuthClientConfig::NAMESPACE, key)
    )
}

/// `raw` parsed, refused naming `setting` when it does not parse.
fn parsed(raw: &str, setting: &str) -> Result<Url, AuthError> {
    Url::parse(raw.trim())
        .map_err(|error| refused(setting, &format!("does not parse as a URL: {error}")))
}

/// The refusal of `setting`, saying `why`.
fn refused(setting: &str, why: &str) -> AuthError {
    AuthError::Failed(format!("{setting}, {why}"))
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

    fn refusal(config: &OAuthClientConfig) -> String {
        let Err(AuthError::Failed(sentence)) = config.refuse_plaintext() else {
            panic!("the config is refused");
        };
        sentence
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

    #[test]
    fn a_loopback_redirect_is_accepted_on_every_spelling() {
        for redirect in [
            "http://localhost:3000/callback",
            "http://127.0.0.1:3000/callback",
            "http://[::1]:3000/callback",
            "https://app.example/callback",
        ] {
            let config = OAuthClientConfig {
                redirect_url: redirect.into(),
                ..complete()
            };
            assert!(config.refuse_plaintext().is_ok(), "{redirect}");
        }
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
        complete().refuse_plaintext().expect("every URL is https");
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

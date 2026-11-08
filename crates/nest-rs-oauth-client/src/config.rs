//! [`OAuthClientConfig`] — env-driven OAuth2 provider endpoints.

use nest_rs_authn::AuthError;
use nest_rs_config::__private::{RetiredKey, RetiredKeys, is_loopback};
use nest_rs_config::{ClientTls, ConfigService, Environment, Field, config, endpoint_shown};
use nest_rs_core::Secret;
use nest_rs_guards::__private::is_scope_token;
use oauth2::url::Url;

use crate::AuthorizationParameters;

// A family puts its name first in its namespace: 6.x read `oauth_client`.
nest_rs_core::inventory::submit! {
    RetiredKeys::under("oauth_client", &[
        RetiredKey::moved("CLIENT_ID", "oauth__client"),
        RetiredKey::moved("CLIENT_SECRET", "oauth__client"),
        RetiredKey::moved("AUTH_URL", "oauth__client"),
        RetiredKey::moved("TOKEN_URL", "oauth__client"),
        RetiredKey::moved("REDIRECT_URL", "oauth__client"),
        RetiredKey::moved("USERINFO_URL", "oauth__client"),
        RetiredKey::moved("SCOPES", "oauth__client"),
    ])
}

/// Env-driven OAuth2 provider endpoints (namespace `oauth__client`); every URL and
/// credential is required.
///
/// The provider's endpoints are `https` URLs, and the redirect an `https` URL,
/// or `http` on a loopback address in a declared development or test profile:
/// [`OAuthClient::new`](crate::OAuthClient::new) refuses any other, naming the
/// setting and the RFC that requires TLS there.
///
/// `Debug` shows each URL through [`endpoint_shown`]: a userinfo or query part
/// may carry a provider's API key.
#[config(namespace = "oauth__client")]
#[derive(Clone, Default)]
pub struct OAuthClientConfig {
    /// The registered OAuth2 client id.
    #[validate(length(min = 1), custom(function = "not_blank"))]
    pub client_id: String,
    /// The registered OAuth2 client secret.
    #[validate(length(min = 1), custom(function = "not_blank_secret"))]
    pub client_secret: Secret<String>,
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
    /// Scopes requested at authorization; empty by default. Each is an RFC 6749
    /// §3.3 `scope-token`, so the request's space-joined list names the scopes
    /// listed: one holding a space, a `"`, a `\` or anything but printable
    /// ASCII fails the boot.
    #[validate(custom(function = "OAuthClientConfig::validate_scopes"))]
    pub scopes: Vec<String>,
    /// What the authorization request carries beside the client's own
    /// parameters (key `AUTHORIZATION_PARAMETERS`, a JSON object of strings) —
    /// `{"access_type":"offline","prompt":"consent"}` for a refresh token from
    /// Google; none by default. A parameter the client sends itself fails the
    /// boot ([`AuthorizationParameters`]).
    #[config(json)]
    #[validate(custom(function = "OAuthClientConfig::validate_authorization_parameters"))]
    pub authorization_parameters: AuthorizationParameters,
    /// What the provider's certificate must chain to — the system's authorities
    /// unless `<PREFIX>_OAUTH__CLIENT__TLS_CA_CERT` names others, which replace
    /// them — and the certificate presented to a provider requiring one
    /// (`TLS_CERT` and `TLS_KEY`). The certificate is verified either way.
    pub tls: ClientTls,
}

impl std::fmt::Debug for OAuthClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            client_id,
            client_secret,
            auth_url,
            token_url,
            redirect_url,
            userinfo_url,
            scopes,
            authorization_parameters,
            tls,
        } = self;
        f.debug_struct("OAuthClientConfig")
            .field("client_id", client_id)
            .field("client_secret", client_secret)
            .field("auth_url", &endpoint_shown(auth_url))
            .field("token_url", &endpoint_shown(token_url))
            .field("redirect_url", &endpoint_shown(redirect_url))
            .field("userinfo_url", &endpoint_shown(userinfo_url))
            .field("scopes", scopes)
            .field("authorization_parameters", authorization_parameters)
            .field("tls", tls)
            .finish()
    }
}

/// A required value holding only whitespace is refused like an empty one.
fn not_blank(value: &str) -> Result<(), validator::ValidationError> {
    if value.trim().is_empty() {
        return Err(validator::ValidationError::new("blank"));
    }
    Ok(())
}

/// [`not_blank`] for a credential held as a [`Secret`].
fn not_blank_secret(value: &Secret<String>) -> Result<(), validator::ValidationError> {
    not_blank(value.expose_secret())
}

impl OAuthClientConfig {
    /// The rule [`scopes`](Self::scopes) is held to, for a provider's config
    /// holding scopes of its own, so its boot names its own variable:
    /// `not_a_scope_token` unless every scope is an RFC 6749 §3.3 `scope-token`.
    ///
    /// ```
    /// use nest_rs_config::{Config, ConfigService, config, var_name};
    /// use nest_rs_oauth_client::{AuthorizationParameters, OAuthClientConfig};
    ///
    /// #[config(namespace = "social__acme")]
    /// #[derive(Clone, Debug, Default)]
    /// pub struct AcmeSocialConfig {
    ///     #[validate(custom(function = "OAuthClientConfig::validate_scopes"))]
    ///     pub scopes: Vec<String>,
    ///     #[config(json)]
    ///     #[validate(custom(function = "OAuthClientConfig::validate_authorization_parameters"))]
    ///     pub authorization_parameters: AuthorizationParameters,
    /// }
    ///
    /// let env = ConfigService::with_vars("social__acme", [("SCOPES", "openid email")]);
    /// let refused = AcmeSocialConfig::from_env(&env, AcmeSocialConfig::default())?
    ///     .check()
    ///     .expect_err("two scopes in one entry");
    /// let said = refused.to_string();
    /// assert!(said.contains(&var_name("social__acme", "SCOPES")), "{said}");
    /// assert!(said.contains("not_a_scope_token"), "{said}");
    /// # Ok::<(), nest_rs_config::ConfigError>(())
    /// ```
    pub fn validate_scopes(scopes: &[String]) -> Result<(), validator::ValidationError> {
        if scopes.iter().all(|scope| is_scope_token(scope)) {
            Ok(())
        } else {
            Err(validator::ValidationError::new("not_a_scope_token"))
        }
    }

    /// The rule [`authorization_parameters`](Self::authorization_parameters) is
    /// held to, for a provider's config holding parameters of its own, as
    /// [`validate_scopes`](Self::validate_scopes) shows: the first fault
    /// [`AuthorizationParameters`] names, `sent_by_the_client`,
    /// `not_a_parameter_name`, `no_value` or `named_twice`.
    pub fn validate_authorization_parameters(
        parameters: &AuthorizationParameters,
    ) -> Result<(), validator::ValidationError> {
        match parameters.refusal() {
            Some(code) => Err(validator::ValidationError::new(code)),
            None => Ok(()),
        }
    }

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
                Self::AUTH_URL,
                "RFC 6749 §3.1 requires TLS at the authorization endpoint, where the user signs \
                 in",
            ),
            (
                &self.token_url,
                Self::TOKEN_URL,
                "RFC 6749 §3.2 requires TLS at the token endpoint, which receives the client \
                 secret and the authorization code",
            ),
            (
                &self.userinfo_url,
                Self::USERINFO_URL,
                "RFC 6750 §5.3 sends a bearer token over TLS alone, and the userinfo endpoint \
                 receives one",
            ),
        ];
        for (raw, field, why) in endpoints {
            let setting = setting(variables, field);
            if parsed(raw, &setting)?.scheme() != "https" {
                return Err(refused(&setting, &format!("must be an https URL — {why}")));
            }
        }
        refuse_plaintext_redirect_in(
            &self.redirect_url,
            &setting(variables, Self::REDIRECT_URL),
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
/// use nest_rs_config::spellings_or_field;
/// use nest_rs_oauth_client::refuse_plaintext_redirect;
///
/// let setting =
///     spellings_or_field("social__acme", "REDIRECT_URL", "AcmeSocialConfig::redirect_url");
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
    let local = redirect.scheme() == "http"
        && redirect.host_str().is_some_and(is_loopback)
        && !environment.is_deployed();
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
fn setting(variables: Option<&ConfigService>, field: Field) -> String {
    match variables {
        Some(variables) => field.spellings_or_field(variables.namespace()),
        None => format!("`{}`", field.path()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OAuthClient;
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
            authorization_parameters: AuthorizationParameters::default(),
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
    fn debug_never_prints_the_client_secret() {
        let printed = format!("{:?}", complete());
        assert!(printed.contains("client_secret: <redacted>"), "{printed}");
        assert!(!printed.contains("\"secret\""), "{printed}");
    }

    /// A provider URL may carry a key in its userinfo or query: `Debug` shows
    /// each one's scheme, host and path alone.
    #[test]
    fn debug_shows_each_url_without_its_userinfo_or_query() {
        let printed = format!(
            "{:?}",
            OAuthClientConfig {
                auth_url: "https://u:URLSECRET1@auth.example/authorize?key=URLSECRET2".into(),
                token_url: "https://auth.example/token?api_key=URLSECRET3".into(),
                redirect_url: "https://u:URLSECRET4@app.example/callback?k=URLSECRET5".into(),
                userinfo_url: "https://user:URLSECRET6@auth.example/userinfo".into(),
                ..complete()
            }
        );
        assert!(!printed.contains("URLSECRET"), "{printed}");
        for shown in [
            "auth_url: \"https://auth.example/authorize\"",
            "token_url: \"https://auth.example/token\"",
            "redirect_url: \"https://app.example/callback\"",
            "userinfo_url: \"https://auth.example/userinfo\"",
        ] {
            assert!(printed.contains(shown), "{shown} in {printed}");
        }
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
            (|c| c.client_secret = Secret::default(), "client_secret"),
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

    /// RFC 6749 §3.3: the request joins the scopes with spaces, so `"a b"` asks
    /// for two and `a"b` for none the grammar allows; the boot refuses both,
    /// naming the key and quoting neither.
    #[test]
    fn a_scope_outside_the_token_grammar_is_refused_naming_the_key() {
        for scope in [
            "never-shown two",
            "",
            "never\"shown",
            "never\\shown",
            "nevér",
        ] {
            let config = OAuthClientConfig {
                scopes: vec!["read".into(), scope.into()],
                ..complete()
            };
            let err = config.validate().expect_err("refused");
            let codes: Vec<_> = err.field_errors()["scopes"]
                .iter()
                .map(|error| error.code.to_string())
                .collect();
            assert_eq!(codes, ["not_a_scope_token"], "{scope:?}");

            let Err(refused) = OAuthClient::from_variables(config) else {
                panic!("{scope:?} boots");
            };
            let said = nest_rs_core::error_message(&refused);
            assert!(
                said.contains(&format!(
                    "scopes ({}): not_a_scope_token",
                    nest_rs_config::var_name("oauth__client", "SCOPES")
                )),
                "{said}"
            );
            assert!(!said.contains("never") && !said.contains("nevér"), "{said}");
        }
    }

    /// `AUTHORIZATION_PARAMETERS`, read as a JSON object, rendered as a boot
    /// refusal would say it.
    fn parameters_refusal(json: &str) -> String {
        let env = ConfigService::with_vars(
            OAuthClientConfig::NAMESPACE,
            [("AUTHORIZATION_PARAMETERS", json)],
        );
        let config = <OAuthClientConfig as nest_rs_config::Config>::from_env(&env, complete())
            .expect("the variable reads");
        let errors = config.validate().expect_err("refused");
        nest_rs_config::ConfigError::validation(OAuthClientConfig::NAMESPACE, errors).to_string()
    }

    /// What this client sends itself, or what decides how the provider reads the
    /// request or answers it, is the client's: a deployment setting one — in any
    /// case — would send it twice or move the answer where no callback reads it.
    /// Its credentials and its verifier are sent at the token endpoint alone: in
    /// the authorization URL, the `Location` every browser is handed, a secret
    /// reaches its history, a `Referer` and the provider's access log, which RFC
    /// 6749 §2.3.1 forbids.
    #[test]
    fn a_parameter_the_client_sends_itself_is_refused_naming_the_key() {
        for owned in [
            "response_type",
            "client_id",
            "redirect_uri",
            "scope",
            "state",
            "code_challenge",
            "code_challenge_method",
            "nonce",
            "response_mode",
            "request",
            "request_uri",
            "client_secret",
            "client_assertion",
            "client_assertion_type",
            "code_verifier",
            "STATE",
            "Client_Secret",
        ] {
            let rendered = parameters_refusal(&format!(r#"{{"{owned}":"never-shown"}}"#));
            assert!(
                rendered.contains(&format!(
                    "authorization_parameters ({}): sent_by_the_client",
                    nest_rs_config::var_name("oauth__client", "AUTHORIZATION_PARAMETERS")
                )),
                "{owned}: {rendered}"
            );
            assert!(!rendered.contains("never-shown"), "{rendered}");

            let composed = OAuthClientConfig {
                authorization_parameters: AuthorizationParameters::from([(owned, "never-shown")]),
                ..complete()
            };
            assert!(
                OAuthClient::from_variables(composed).is_err(),
                "{owned}: a client is built to send it"
            );
        }
    }

    /// RFC 6749 §8.2's `param-name`, a value to send (§3.1: one sent without a
    /// value is as if omitted), and each parameter once (§3.1 again), in any
    /// case, since a provider may read a name loosely.
    #[test]
    fn a_parameter_no_request_can_carry_is_refused_by_what_is_wrong() {
        for (json, code) in [
            (r#"{"never shown":"offline"}"#, "not_a_parameter_name"),
            (r#"{"":"offline"}"#, "not_a_parameter_name"),
            (r#"{"never=shown":"offline"}"#, "not_a_parameter_name"),
            (r#"{"access_type":""}"#, "no_value"),
            (
                r#"{"prompt":"consent","prompt":"never-shown"}"#,
                "named_twice",
            ),
            (
                r#"{"prompt":"consent","PROMPT":"never-shown"}"#,
                "named_twice",
            ),
        ] {
            let rendered = parameters_refusal(json);
            assert!(
                rendered.contains(&format!(
                    "authorization_parameters ({}): {code}",
                    nest_rs_config::var_name("oauth__client", "AUTHORIZATION_PARAMETERS")
                )),
                "{json}: {rendered}"
            );
            assert!(!rendered.contains("never"), "{rendered}");
        }
    }

    /// A provider's config holds the fields it hands this client by the client's
    /// own rules, so its boot names its own variable.
    #[test]
    fn a_provider_s_config_holds_its_fields_by_the_client_s_rules() {
        assert!(OAuthClientConfig::validate_scopes(&["openid".into(), "email".into()]).is_ok());
        let refused = OAuthClientConfig::validate_scopes(&["openid email".into()])
            .expect_err("two scopes in one");
        assert_eq!(refused.code, "not_a_scope_token");

        let consent = AuthorizationParameters::from([("prompt", "consent")]);
        assert!(OAuthClientConfig::validate_authorization_parameters(&consent).is_ok());
        let credential = AuthorizationParameters::from([("client_secret", "never-shown")]);
        let refused = OAuthClientConfig::validate_authorization_parameters(&credential)
            .expect_err("a credential");
        assert_eq!(refused.code, "sent_by_the_client");
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

//! [`AuthorizationParameters`] — what an authorization request carries beside
//! the parameters the client sends itself.

use std::fmt;

use serde::de::{Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeMap as _, Serializer};
use serde::{Deserialize, Serialize};

/// Parameters an authorization request (RFC 6749 §4.1.1) carries beside the ones
/// [`OAuthClient`](crate::OAuthClient) sends itself, each under the name the
/// provider reads: Google's `access_type=offline` to be issued a refresh token and
/// `prompt=consent` to ask the consent again, OpenID Connect Core's `prompt`,
/// `max_age` or `ui_locales`, GitHub's `prompt=select_account` or `allow_signup`.
///
/// A deployment sets it as a JSON object of strings, one parameter each:
///
/// ```
/// use nest_rs_config::{Config, ConfigService};
/// use nest_rs_oauth_client::{AuthorizationParameters, OAuthClientConfig};
///
/// let env = ConfigService::with_vars(
///     "oauth__client",
///     [("AUTHORIZATION_PARAMETERS", r#"{"access_type":"offline","prompt":"consent"}"#)],
/// );
/// let config = OAuthClientConfig::from_env(&env, OAuthClientConfig::default())?;
/// assert_eq!(
///     config.authorization_parameters,
///     AuthorizationParameters::from([("access_type", "offline"), ("prompt", "consent")]),
/// );
/// # Ok::<(), nest_rs_config::ConfigError>(())
/// ```
///
/// The boot refuses, naming the variable and never a value, a parameter the client
/// sends or reads itself, in any case: `response_type`, `client_id`,
/// `redirect_uri`, `scope`, `state`, `code_challenge`, `code_challenge_method`,
/// `nonce`, `response_mode`, `request` and `request_uri`, then what it sends the
/// token endpoint alone, `client_secret`, `client_assertion`,
/// `client_assertion_type` and `code_verifier`. Sent twice, a provider may read
/// either; `response_mode`, `request` or `request_uri` would move the answer, or
/// the request, where the callback reads nothing; and a credential in the
/// authorization URL reaches every browser's history and the provider's access
/// log, which RFC 6749 §2.3.1 forbids. It refuses a name outside RFC 6749 §8.2's
/// grammar, a parameter with no value, which §3.1 treats as omitted, and one
/// named twice in any case, which §3.1 forbids
/// ([`OAuthClientConfig::validate_authorization_parameters`](crate::OAuthClientConfig::validate_authorization_parameters),
/// which a provider's config holding its own validates them with).
///
/// `Debug` prints the names alone: a value is the deployment's, and a provider may
/// read a key in one.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AuthorizationParameters(Vec<(String, String)>);

impl AuthorizationParameters {
    /// What the client sends or reads itself: RFC 6749 §4.1.1's request, RFC 7636
    /// §4.3's challenge, OpenID Connect Core §3.1.2.1's `nonce`, the response mode
    /// of OAuth 2.0 Multiple Response Type Encoding Practices §2.1 and RFC 9101's
    /// request object; then what it sends the token endpoint alone, its
    /// credentials (RFC 6749 §2.3.1, RFC 7521 §4.2) and RFC 7636 §4.5's verifier.
    const SENT_BY_THE_CLIENT: &'static [&'static str] = &[
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
    ];

    /// Each parameter, by its name and its value, in the order written.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// The validation code the boot refuses these parameters with, the first
    /// fault found — `sent_by_the_client`, `not_a_parameter_name`, `no_value` or
    /// `named_twice` — or `None`.
    pub(crate) fn refusal(&self) -> Option<&'static str> {
        for (at, (name, value)) in self.0.iter().enumerate() {
            if Self::SENT_BY_THE_CLIENT
                .iter()
                .any(|owned| owned.eq_ignore_ascii_case(name))
            {
                return Some("sent_by_the_client");
            }
            if !is_parameter_name(name) {
                return Some("not_a_parameter_name");
            }
            if value.is_empty() {
                return Some("no_value");
            }
            if self.0[..at]
                .iter()
                .any(|(earlier, _)| earlier.eq_ignore_ascii_case(name))
            {
                return Some("named_twice");
            }
        }
        None
    }
}

/// RFC 6749 §8.2: `param-name = 1*name-char`, `name-char = "-" / "." / "_" /
/// DIGIT / ALPHA`.
fn is_parameter_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_'))
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for AuthorizationParameters {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(parameters: I) -> Self {
        Self(
            parameters
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        )
    }
}

impl<K: Into<String>, V: Into<String>, const N: usize> From<[(K, V); N]>
    for AuthorizationParameters
{
    fn from(parameters: [(K, V); N]) -> Self {
        parameters.into_iter().collect()
    }
}

impl fmt::Debug for AuthorizationParameters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AuthorizationParameters")
            .field(&self.0.iter().map(|(name, _)| name).collect::<Vec<_>>())
            .finish()
    }
}

/// The JSON object a deployment writes.
impl Serialize for AuthorizationParameters {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, value) in &self.0 {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

/// Read as written, a name given twice kept twice, so the boot refuses it rather
/// than a map keeping the last one in silence.
impl<'de> Deserialize<'de> for AuthorizationParameters {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Parameters;

        impl<'de> Visitor<'de> for Parameters {
            type Value = AuthorizationParameters;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object of authorization request parameters, each a string")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut parameters = Vec::new();
                while let Some(parameter) = map.next_entry::<String, String>()? {
                    parameters.push(parameter);
                }
                Ok(AuthorizationParameters(parameters))
            }
        }

        deserializer.deserialize_map(Parameters)
    }
}

#[cfg(test)]
mod tests {
    use super::AuthorizationParameters;

    fn refusal(parameters: &[(&str, &str)]) -> Option<&'static str> {
        parameters
            .iter()
            .copied()
            .collect::<AuthorizationParameters>()
            .refusal()
    }

    #[test]
    fn a_provider_s_own_parameters_pass() {
        assert_eq!(
            refusal(&[
                ("access_type", "offline"),
                ("prompt", "consent"),
                ("max_age", "0"),
                ("hd", "example.com"),
                ("ui.locales-x", "fr"),
            ]),
            None
        );
        assert_eq!(AuthorizationParameters::default().refusal(), None);
    }

    #[test]
    fn a_name_outside_the_grammar_is_refused() {
        for name in ["", "a b", "a=b", "a&b", "é", "a/b"] {
            assert_eq!(
                refusal(&[(name, "x")]),
                Some("not_a_parameter_name"),
                "{name:?}"
            );
        }
    }

    #[test]
    fn debug_prints_the_names_alone() {
        let printed = format!(
            "{:?}",
            AuthorizationParameters::from([("access_type", "never-shown")])
        );
        assert_eq!(printed, r#"AuthorizationParameters(["access_type"])"#);
    }
}

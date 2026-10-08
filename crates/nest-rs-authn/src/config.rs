//! [`AuthnConfig`] — env-driven JWT key material.

use std::time::Duration;

use jsonwebtoken::Algorithm;
use nest_rs_config::{Bound, Config, ConfigService, DurationBounds, Floor, Namespaced, config};

use crate::error::AuthError;
use crate::service::{FAMILIES, algorithms_of, one_of};
use crate::{AuthnTls, JwtKey, JwtOptions};

/// The token lifetime's range, the variable that sets it, and why.
pub(crate) const EXPIRES_IN: DurationBounds = DurationBounds::secs(
    "EXPIRES_IN_SECS",
    "AuthnConfig::expires_in_secs",
    Floor::Units(Bound {
        count: 1,
        why: "a token that expires as it is minted is refused by every verifier, so every \
              sign-in would succeed and hand out nothing usable",
    }),
    Bound {
        count: 30 * 24 * 60 * 60,
        why: "an access token is a bearer credential until it expires and nothing revokes it \
              sooner, so a lifetime past thirty days is a unit slip or a credential that never \
              ends — a long session is a refresh flow, not a long token",
    },
);

/// The clock-skew leeway's range, the variable that sets it, and why.
pub(crate) const LEEWAY: DurationBounds = DurationBounds::secs(
    "LEEWAY_SECS",
    "AuthnConfig::leeway_secs",
    Floor::Units(Bound {
        count: 0,
        why: "no leeway is a leeway",
    }),
    Bound {
        count: 5 * 60,
        why: "RFC 7519 §4.1.4 allows \"no more than a few minutes\" for clock skew, and every \
              second past it is a second an expired token still verifies",
    },
);

/// Env-driven JWT key material (namespace `authn`). The combination of keys
/// present selects the signing mode; see [`into_options`](Self::into_options).
/// No `Debug` derive: secrets must not leak through a format.
#[config(namespace = "authn")]
#[derive(Clone, Default)]
pub struct AuthnConfig {
    /// HS256 shared secret (key `SECRET`); must be ≥ 32 bytes. A verifier
    /// holding it can also mint tokens. Refused beside either EdDSA key.
    pub secret: Option<String>,
    /// EdDSA signing key, PEM (key `PRIVATE_KEY`, or a path in
    /// `PRIVATE_KEY_FILE`). Set only on the app that issues tokens, beside the
    /// `public_key` of its own pair.
    pub private_key: Option<String>,
    /// EdDSA verification key, PEM (key `PUBLIC_KEY`, or a path in
    /// `PUBLIC_KEY_FILE`). A resource server holds only this — it can verify
    /// but not sign.
    pub public_key: Option<String>,
    /// The URI of the JWK Set (RFC 7517 §5) an external issuer publishes its
    /// keys at (key `JWKS_URI`), `https` only — the `jwks_uri` of its RFC 8414
    /// or OpenID Connect metadata. Verify-only, and refused beside `secret`,
    /// `private_key` or `public_key`.
    pub jwks_uri: Option<String>,
    /// The algorithms a token may be signed with (key `ALGORITHMS`,
    /// comma-separated, `RS256,EdDSA`). Unset, a secret is HS256, a PEM key
    /// EdDSA, and a JWK Set every asymmetric algorithm; a secret or a PEM key
    /// takes exactly one.
    pub algorithms: Option<Vec<Algorithm>>,
    /// What the JWK Set endpoint's certificate must chain to — the system's
    /// authorities unless `<PREFIX>_AUTHN__TLS_CA_CERT` names one. Refused
    /// without `jwks_uri`, which is the only connection it could serve.
    pub tls: AuthnTls,
    /// Clock skew leeway in seconds (key `LEEWAY_SECS`, default 30), at most
    /// 300 — RFC 7519 §4.1.4's "a few minutes".
    pub leeway_secs: Option<u64>,
    /// Expected `aud` claim (key `AUDIENCE`). Set ⇒ the claim is **mandatory**
    /// and must name this service.
    ///
    /// Unset, a token whose `aud` does not name this service is still refused
    /// (RFC 7519 §4.1.3); set, a token with no `aud` is refused too. The opt-out
    /// is [`allow_any_audience`](Self::allow_any_audience), and only that.
    pub audience: Option<String>,
    /// Expected `iss` claim (key `ISSUER`). Omitted ⇒ no issuer check.
    pub issuer: Option<String>,
    /// Token lifetime in seconds (key `EXPIRES_IN_SECS`, default 3600), at
    /// least 1 and at most thirty days.
    pub expires_in_secs: Option<u64>,
    /// Opt out of RFC 7519 §4.1.3 (key `ALLOW_ANY_AUDIENCE`, default `false`):
    /// accept a token whose `aud` names a service this one is not.
    ///
    /// Restores the confused deputy, so it is refused beside
    /// [`audience`](Self::audience) and reports itself at `warn` once per boot.
    pub allow_any_audience: bool,
    /// RFC 9068 explicit typing — stamp `typ: at+jwt` when minting and refuse a
    /// token typed anything else when verifying. Defaults to **on**: §4 states
    /// the verifier's half as a MUST, and §2.1 names what it prevents — an
    /// OpenID Connect ID Token being spent as an access token.
    ///
    /// Turn it off only to verify tokens from an issuer that predates the
    /// profile and mints a plain `typ: JWT`.
    pub explicit_typing: Option<bool>,
}

impl Config for AuthnConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            secret: env.get("SECRET")?.or(base.secret),
            private_key: pem_text(env, "PRIVATE_KEY")?.or(base.private_key),
            public_key: pem_text(env, "PUBLIC_KEY")?.or(base.public_key),
            jwks_uri: env.get("JWKS_URI")?.or(base.jwks_uri),
            algorithms: algorithms(env)?.or(base.algorithms),
            tls: AuthnTls::from_env(env, base.tls)?,
            leeway_secs: seconds(LEEWAY.read_optional(env, base.leeway_secs.map(secs))?),
            audience: env.get("AUDIENCE")?.or(base.audience),
            issuer: env.get("ISSUER")?.or(base.issuer),
            expires_in_secs: seconds(
                EXPIRES_IN.read_optional(env, base.expires_in_secs.map(secs))?,
            ),
            allow_any_audience: env.flag("ALLOW_ANY_AUDIENCE", base.allow_any_audience)?,
            explicit_typing: match env
                .flag("EXPLICIT_TYPING", base.explicit_typing.unwrap_or(true))?
            {
                // Round-trip through the base so an unset variable stays
                // "unstated" rather than becoming a pinned `true`.
                v if base.explicit_typing.is_none() && v => None,
                v => Some(v),
            },
        })
    }
}

fn secs(count: u64) -> Duration {
    Duration::from_secs(count)
}

/// A bounded read back in the whole seconds the field holds.
fn seconds(read: Option<nest_rs_config::BoundedDuration>) -> Option<u64> {
    read.map(|read| read.value.as_secs())
}

/// EdDSA key material for `key` — inline, or read from the file `<KEY>_FILE`
/// names — as the text the JWT library parses.
fn pem_text(env: &ConfigService, key: &str) -> nest_rs_config::Result<Option<String>> {
    let Some(pem) = env.material(key)? else {
        return Ok(None);
    };
    match std::str::from_utf8(&pem.value.bytes) {
        Ok(text) => Ok(Some(text.to_owned())),
        Err(_) => Err(pem.refuse("is not PEM text: it holds bytes that are not UTF-8")),
    }
}

/// The `ALGORITHMS` list, each name read as RFC 7518 spells it — case and all,
/// since a JOSE `alg` is case-sensitive. A name it does not know, `none`
/// included, fails the boot; whether the list fits the key is
/// [`JwtService::new`](crate::JwtService::new)'s to judge.
fn algorithms(env: &ConfigService) -> nest_rs_config::Result<Option<Vec<Algorithm>>> {
    let Some(setting) = env.setting("ALGORITHMS")? else {
        return Ok(None);
    };
    let known = || one_of(algorithms_of(FAMILIES));
    setting
        .value
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| {
            // The parser's error says no more than that the name is unknown.
            name.parse::<Algorithm>().ok().ok_or_else(|| {
                if setting.from_file() {
                    setting.refuse(format_args!(
                        "names an algorithm this verifier does not run: use {}",
                        known()
                    ))
                } else {
                    setting.refuse(format_args!(
                        "`{name}` is not an algorithm this verifier runs: use {}",
                        known()
                    ))
                }
            })
        })
        .collect::<nest_rs_config::Result<Vec<_>>>()
        .map(Some)
}

/// A PEM key's setting, every way it can be given: which spelling supplied a
/// field is unknown once an [`AuthnConfig`] is judged. A sentence continuing
/// past it sets it off with commas.
pub(crate) fn spellings(key: &str, field: &str) -> String {
    format!(
        "{}, or `{field}` in an AuthnConfig or JwtOptions built in code",
        nest_rs_config::spellings(AuthnConfig::NAMESPACE, key),
    )
}

/// The secret's setting, every way it can be given, `_FILE` spelling included.
pub(crate) fn secret_setting() -> String {
    spellings("SECRET", "secret")
}

/// The JWK Set URI's setting, every way it can be given.
pub(crate) fn jwks_uri_setting() -> String {
    spellings("JWKS_URI", "jwks_uri")
}

/// The accepted algorithms' setting, every way it can be given.
pub(crate) fn algorithms_setting() -> String {
    spellings("ALGORITHMS", "algorithms")
}

/// The JWK Set endpoint's authority setting, `_FILE` spelling included.
pub(crate) fn tls_ca_cert_setting() -> String {
    spellings("TLS_CA_CERT", "tls.ca_cert")
}

/// The key a secret, a private key and a public key make, or why they make
/// none — every combination named, the order of the arms deciding nothing.
fn static_key(
    secret: Option<String>,
    private: Option<String>,
    public: Option<String>,
) -> Result<JwtKey, AuthError> {
    match (secret, private, public) {
        (Some(secret), None, None) => Ok(JwtKey::Hmac(secret)),
        (Some(_), private @ Some(_), public) | (Some(_), private @ None, public @ Some(_)) => {
            let keys = [
                private
                    .as_ref()
                    .map(|_| spellings("PRIVATE_KEY", "private_key")),
                public
                    .as_ref()
                    .map(|_| spellings("PUBLIC_KEY", "public_key")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", and ");
            Err(AuthError::Failed(format!(
                "{}, is set beside {keys}. An HS256 secret and EdDSA keys are two signing modes, \
                 and nothing says which is meant — remove the secret from wherever it was set \
                 to use EdDSA, or the keys to use HS256",
                secret_setting(),
            )))
        }
        (None, private, Some(public)) => Ok(JwtKey::Pem {
            private_pem: private,
            public_pem: public,
        }),
        (None, Some(_), None) => Err(AuthError::Failed(format!(
            "{}, is set without {}",
            spellings("PRIVATE_KEY", "private_key"),
            spellings("PUBLIC_KEY", "public_key"),
        ))),
        (None, None, None) => Err(AuthError::Failed(format!(
            "no JWT key configured: set {} for HS256, {} for EdDSA, or {} for an external \
             issuer's keys — or the same field in an AuthnConfig or JwtOptions built in code",
            nest_rs_config::spellings(AuthnConfig::NAMESPACE, "SECRET"),
            nest_rs_config::spellings(AuthnConfig::NAMESPACE, "PUBLIC_KEY"),
            nest_rs_config::spellings(AuthnConfig::NAMESPACE, "JWKS_URI"),
        ))),
    }
}

impl AuthnConfig {
    /// Infer signing mode from the keys present. Fails the boot when no usable
    /// combination exists.
    ///
    /// A pair signs and verifies EdDSA, a public key alone verifies it, a secret
    /// alone signs and verifies HS256, a JWK Set URI alone verifies an external
    /// issuer's tokens. Refused: a JWK Set URI beside a secret or a key, an
    /// authority without a JWK Set URI, a secret beside either key, a private
    /// key without its public key, nothing set. Whether each value is usable is
    /// [`JwtService::new`](crate::JwtService::new)'s to judge.
    pub fn into_options(self) -> Result<JwtOptions, AuthError> {
        let key = match self.jwks_uri {
            Some(uri) => {
                let beside = [
                    self.secret.as_ref().map(|_| secret_setting()),
                    self.private_key
                        .as_ref()
                        .map(|_| spellings("PRIVATE_KEY", "private_key")),
                    self.public_key
                        .as_ref()
                        .map(|_| spellings("PUBLIC_KEY", "public_key")),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                if !beside.is_empty() {
                    return Err(AuthError::Failed(format!(
                        "{}, is set beside {}. An external issuer's JWK Set and a key of this \
                         deployment's own are two key sources, and nothing says which is meant \
                         — remove the static key from wherever it was set to verify against the \
                         JWK Set, or the JWK Set URI to keep the static key",
                        jwks_uri_setting(),
                        beside.join(", and "),
                    )));
                }
                JwtKey::Jwks { uri, tls: self.tls }
            }
            None => {
                if self.tls.ca_cert.is_some() {
                    return Err(AuthError::Failed(format!(
                        "{}, is set without {}: it names the authority a JWK Set endpoint's \
                         certificate chains to, and no JWK Set is fetched",
                        tls_ca_cert_setting(),
                        jwks_uri_setting(),
                    )));
                }
                static_key(self.secret, self.private_key, self.public_key)?
            }
        };
        let mut options = JwtOptions::with_key(key);
        if let Some(algorithms) = self.algorithms {
            options.algorithms = algorithms;
        }
        options.leeway = Duration::from_secs(self.leeway_secs.unwrap_or(30));
        options.audience = self.audience;
        options.issuer = self.issuer;
        options.allow_any_audience = self.allow_any_audience;
        options.explicit_typing = self.explicit_typing.unwrap_or(true);
        if let Some(secs) = self.expires_in_secs {
            options.expires_in = Duration::from_secs(secs);
        }
        Ok(options)
    }
}

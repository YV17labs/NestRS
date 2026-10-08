//! [`AuthnConfig`] — env-driven JWT key material.

use std::time::Duration;

use jsonwebtoken::Algorithm;
use nest_rs_config::{Bound, Config, ConfigService, DurationBounds, Floor, Namespaced, config};

use crate::error::AuthError;
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

/// Every JWS algorithm this verifier runs, in the order a refusal lists them.
const ALGORITHM_NAMES: &str =
    "HS256, HS384, HS512, RS256, RS384, RS512, PS256, PS384, PS512, ES256, ES384 or EdDSA";

/// The `ALGORITHMS` list, each name read as RFC 7518 spells it — case and all,
/// since a JOSE `alg` is case-sensitive. A name it does not know, `none`
/// included, fails the boot.
fn algorithms(env: &ConfigService) -> nest_rs_config::Result<Option<Vec<Algorithm>>> {
    let Some(setting) = env.setting("ALGORITHMS")? else {
        return Ok(None);
    };
    let mut algorithms = Vec::new();
    for name in setting
        .value
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        // The parser's error says no more than that the name is unknown.
        let algorithm = name.parse::<Algorithm>().ok().ok_or_else(|| {
            if setting.from_file() {
                setting.refuse(format_args!(
                    "names an algorithm this verifier does not run: use {ALGORITHM_NAMES}"
                ))
            } else {
                setting.refuse(format_args!(
                    "`{name}` is not an algorithm this verifier runs: use {ALGORITHM_NAMES}"
                ))
            }
        })?;
        algorithms.push(algorithm);
    }
    if algorithms.is_empty() {
        return Err(setting.refuse(format_args!(
            "names no algorithm: use {ALGORITHM_NAMES}, comma-separated"
        )));
    }
    Ok(Some(algorithms))
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

impl AuthnConfig {
    /// Infer signing mode from the keys present. Fails the boot when no usable
    /// combination exists.
    ///
    /// A pair signs and verifies EdDSA, a public key alone verifies it, a secret
    /// alone signs and verifies HS256, a JWK Set URI alone verifies an external
    /// issuer's tokens. Refused in order: a JWK Set URI beside a secret or a
    /// key, a secret beside either key, a private key without its public key,
    /// an authority without a JWK Set URI, nothing set. Whether each value is
    /// usable is [`JwtService::new`](crate::JwtService::new)'s to judge.
    pub fn into_options(self) -> Result<JwtOptions, AuthError> {
        let leeway = Duration::from_secs(self.leeway_secs.unwrap_or(30));
        let audience = self.audience;
        let (secret, private, public) = (self.secret, self.private_key, self.public_key);
        if self.jwks_uri.is_some() {
            let keys = [
                secret.as_ref().map(|_| secret_setting()),
                private
                    .as_ref()
                    .map(|_| spellings("PRIVATE_KEY", "private_key")),
                public
                    .as_ref()
                    .map(|_| spellings("PUBLIC_KEY", "public_key")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            if !keys.is_empty() {
                return Err(AuthError::Failed(format!(
                    "{}, is set beside {}. An external issuer's JWK Set and a key of this \
                     deployment's own are two key sources, and nothing says which is meant — \
                     remove the static key from wherever it was set to verify against the JWK \
                     Set, or the JWK Set URI to keep the static key",
                    spellings("JWKS_URI", "jwks_uri"),
                    keys.join(", and "),
                )));
            }
        }
        if secret.is_some() && (private.is_some() || public.is_some()) {
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
            return Err(AuthError::Failed(format!(
                "{}, is set beside {keys}. An HS256 secret and EdDSA keys are two signing modes, \
                 and nothing says which is meant — remove the secret from wherever it was set \
                 to use EdDSA, or the keys to use HS256",
                secret_setting(),
            )));
        }
        if private.is_some() && public.is_none() {
            return Err(AuthError::Failed(format!(
                "{}, is set without {}",
                spellings("PRIVATE_KEY", "private_key"),
                spellings("PUBLIC_KEY", "public_key"),
            )));
        }
        if self.tls.ca_cert.is_some() && self.jwks_uri.is_none() {
            return Err(AuthError::Failed(format!(
                "{}, is set without {}: it names the authority a JWK Set endpoint's certificate \
                 chains to, and no JWK Set is fetched",
                spellings("TLS_CA_CERT", "tls.ca_cert"),
                spellings("JWKS_URI", "jwks_uri"),
            )));
        }
        let mut options = match (secret, private, public, self.jwks_uri) {
            (Some(secret), _, _, _) => JwtOptions::new(secret),
            (None, private, Some(public), _) => match private {
                Some(private) => JwtOptions::eddsa(private, public),
                None => JwtOptions::eddsa_verify(public),
            },
            (None, _, None, Some(uri)) => {
                let mut options = JwtOptions::jwks(uri.clone());
                options.key = JwtKey::Jwks {
                    uri,
                    ca_cert: self.tls.ca_cert,
                };
                options
            }
            // A private key alone was refused above, so only nothing is left.
            (None, _, None, None) => {
                return Err(AuthError::Failed(format!(
                    "no JWT key configured: set {} for HS256, {} for EdDSA, or {} for an \
                     external issuer's keys — or the same field in an AuthnConfig or JwtOptions \
                     built in code",
                    nest_rs_config::spellings(Self::NAMESPACE, "SECRET"),
                    nest_rs_config::spellings(Self::NAMESPACE, "PUBLIC_KEY"),
                    nest_rs_config::spellings(Self::NAMESPACE, "JWKS_URI"),
                )));
            }
        };
        if let Some(algorithms) = self.algorithms {
            options.algorithms = algorithms;
        }
        options.leeway = leeway;
        options.audience = audience;
        options.issuer = self.issuer;
        options.allow_any_audience = self.allow_any_audience;
        options.explicit_typing = self.explicit_typing.unwrap_or(true);
        if let Some(secs) = self.expires_in_secs {
            options.expires_in = Duration::from_secs(secs);
        }
        Ok(options)
    }
}

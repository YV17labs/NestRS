//! [`AuthnConfig`] — env-driven JWT key material.

use std::time::Duration;

use nest_rs_config::{
    Bound, ClientTls, ConfigService, DurationBounds, Field, Floor, Namespaced, config,
    endpoint_shown,
};
use nest_rs_core::Secret;

use crate::algorithm::{Family, algorithms_of};
use crate::error::AuthError;
use crate::service::one_of;
use crate::{Algorithm, JwtKey, JwtOptions};

/// The token lifetime's range, and why.
const EXPIRES_IN: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "a token that expires as it is minted is refused by every verifier, so every \
         sign-in would succeed and hand out nothing usable",
    )),
    Bound::new(
        30 * 24 * 60 * 60,
        "an access token is a bearer credential until it expires and nothing revokes it \
         sooner, so a lifetime past thirty days is a unit slip or a credential that never \
         ends — a long session is a refresh flow, not a long token",
    ),
);

/// The clock-skew leeway's range, and why.
const LEEWAY: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(0, "no leeway is a leeway")),
    Bound::new(
        5 * 60,
        "RFC 7519 §4.1.4 allows \"no more than a few minutes\" for clock skew, and every \
         second past it is a second an expired token still verifies",
    ),
);

/// Env-driven JWT key material (namespace `authn`). The combination of keys
/// present selects the signing mode; see [`into_options`](Self::into_options).
///
/// `Debug` shows `jwks_uri` through [`endpoint_shown`]: its userinfo or query
/// may carry the issuer's credential.
#[config(namespace = "authn")]
#[derive(Clone)]
pub struct AuthnConfig {
    /// HS256 shared secret (key `SECRET`); must be ≥ 32 bytes. A verifier
    /// holding it can also mint tokens. Refused beside either EdDSA key.
    pub secret: Option<Secret<String>>,
    /// EdDSA signing key, PEM (key `PRIVATE_KEY`, or a path in
    /// `PRIVATE_KEY_FILE`). Set only on the app that issues tokens, beside the
    /// `public_key` of its own pair.
    #[config(with = pem_text)]
    pub private_key: Option<Secret<String>>,
    /// EdDSA verification key, PEM (key `PUBLIC_KEY`, or a path in
    /// `PUBLIC_KEY_FILE`). A resource server holds only this — it can verify
    /// but not sign.
    #[config(with = pem_text)]
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
    #[config(with = algorithms)]
    pub algorithms: Option<Vec<Algorithm>>,
    /// What the JWK Set endpoint's certificate must chain to — the system's
    /// authorities unless `<PREFIX>_AUTHN__TLS_CA_CERT` names others — and the
    /// certificate presented to an issuer that requires one
    /// (`<PREFIX>_AUTHN__TLS_CERT` and `TLS_KEY`). Refused without `jwks_uri`,
    /// which is the only connection it could serve.
    pub tls: ClientTls,
    /// Clock skew leeway (key `LEEWAY_SECS`, whole seconds; default 30), at
    /// most 300 — RFC 7519 §4.1.4's "a few minutes".
    #[config(bounds = LEEWAY)]
    pub leeway: Duration,
    /// The `aud` this app requires of every token it verifies (key
    /// `AUDIENCE`), and the one a token it mints for itself carries — a token
    /// it mints for another resource names that resource instead
    /// ([`JwtService::sign_for`](crate::JwtService::sign_for)).
    ///
    /// Required: RFC 9068 §2.2 has every access token carry an `aud` and §4 a
    /// resource server refuse one naming another or none, so unset or blank,
    /// the boot refuses. Under `OAuthResourceModule`, unset is the resource it
    /// serves. The opt-out is [`allow_any_audience`](Self::allow_any_audience),
    /// and only that.
    pub audience: Option<String>,
    /// The `iss` every token this app mints carries and every token it
    /// verifies must name (key `ISSUER`). Required, as RFC 9068 §2.2 and §4
    /// have every access token carry it and a resource server check it: unset
    /// or blank, the boot refuses — but under `OAuthResourceModule`, where a
    /// token's `iss` must be one of its authorization servers and a set one
    /// must be among them, and unset, the app mints no access token.
    pub issuer: Option<String>,
    /// Token lifetime (key `EXPIRES_IN_SECS`, whole seconds; default an hour),
    /// at least 1 second and at most thirty days.
    #[config(bounds = EXPIRES_IN)]
    pub expires_in: Duration,
    /// Opt out of RFC 7519 §4.1.3 (key `ALLOW_ANY_AUDIENCE`, default `false`):
    /// accept a token whose `aud` names a service this one is not.
    ///
    /// Restores the confused deputy, so it is refused beside
    /// [`audience`](Self::audience) and beside `OAuthResourceModule`, and
    /// reports itself at `warn` once per boot. It stands in for the audience
    /// the boot requires, so the app mints no token for itself, only for a
    /// resource it names ([`JwtService::sign_for`](crate::JwtService::sign_for)).
    pub allow_any_audience: bool,
    /// RFC 9068 explicit typing — stamp `typ: at+jwt` when minting and refuse a
    /// token typed anything else when verifying. Defaults to **on**: §4 states
    /// the verifier's half as a MUST, and §2.1 names what it prevents — an
    /// OpenID Connect ID Token being spent as an access token.
    ///
    /// Turn it off only to verify tokens from an issuer that predates the
    /// profile and mints a plain `typ: JWT`. Key `EXPLICIT_TYPING`.
    pub explicit_typing: bool,
}

impl Default for AuthnConfig {
    fn default() -> Self {
        Self {
            secret: None,
            private_key: None,
            public_key: None,
            jwks_uri: None,
            algorithms: None,
            tls: ClientTls::default(),
            leeway: JwtOptions::DEFAULT_LEEWAY,
            audience: None,
            issuer: None,
            expires_in: JwtOptions::DEFAULT_EXPIRES_IN,
            allow_any_audience: false,
            explicit_typing: true,
        }
    }
}

impl std::fmt::Debug for AuthnConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            secret,
            private_key,
            public_key,
            jwks_uri,
            algorithms,
            tls,
            leeway,
            audience,
            issuer,
            expires_in,
            allow_any_audience,
            explicit_typing,
        } = self;
        f.debug_struct("AuthnConfig")
            .field("secret", secret)
            .field("private_key", private_key)
            .field("public_key", public_key)
            .field("jwks_uri", &jwks_uri.as_deref().map(endpoint_shown))
            .field("algorithms", algorithms)
            .field("tls", tls)
            .field("leeway", leeway)
            .field("audience", audience)
            .field("issuer", issuer)
            .field("expires_in", expires_in)
            .field("allow_any_audience", allow_any_audience)
            .field("explicit_typing", explicit_typing)
            .finish()
    }
}

/// EdDSA key material for `key` — inline, or read from the file `<KEY>_FILE`
/// names — as the text the JWT library parses, over `base`.
fn pem_text<T: From<String>>(
    env: &ConfigService,
    field: Field,
    base: Option<T>,
) -> nest_rs_config::Result<Option<T>> {
    let Some(pem) = env.material(field.key())? else {
        return Ok(base);
    };
    match std::str::from_utf8(&pem.value.bytes) {
        Ok(text) => Ok(Some(T::from(text.to_owned()))),
        Err(_) => Err(pem.refuse("is not PEM text: it holds bytes that are not UTF-8")),
    }
}

/// The `ALGORITHMS` list, each name read as RFC 7518 spells it — case and all,
/// since a JOSE `alg` is case-sensitive. A name it does not know, `none`
/// included, fails the boot; whether the list fits the key is
/// [`JwtService::new`](crate::JwtService::new)'s to judge.
fn algorithms(
    env: &ConfigService,
    field: Field,
    base: Option<Vec<Algorithm>>,
) -> nest_rs_config::Result<Option<Vec<Algorithm>>> {
    let Some(setting) = env.setting(field.key())? else {
        return Ok(base);
    };
    let known = || one_of(algorithms_of(Family::ALL));
    setting
        .value
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .enumerate()
        .map(|(index, name)| {
            // The parser's error says no more than that the name is unknown.
            name.parse::<Algorithm>().ok().ok_or_else(|| {
                setting.refuse(format_args!(
                    "entry {} of the list names an algorithm this verifier does not run: use {}",
                    index + 1,
                    known()
                ))
            })
        })
        .collect::<nest_rs_config::Result<Vec<_>>>()
        .map(Some)
}

/// A PEM key's setting, every way it can be given: which spelling supplied a
/// field is unknown once an [`AuthnConfig`] is judged. A sentence continuing
/// past it sets it off with commas.
pub(crate) fn spellings(field: Field) -> String {
    format!(
        "{}, or `{}` in an AuthnConfig or JwtOptions built in code",
        field.spellings(AuthnConfig::NAMESPACE),
        bare(field),
    )
}

/// `field`'s name alone: a sentence naming it speaks of two types, so it names
/// neither path.
fn bare(field: Field) -> &'static str {
    let path = field.path();
    path.rsplit("::").next().unwrap_or(path)
}

/// The secret's setting, every way it can be given, `_FILE` spelling included.
pub(crate) fn secret_setting() -> String {
    spellings(AuthnConfig::SECRET)
}

/// The JWK Set URI's setting, every way it can be given.
pub(crate) fn jwks_uri_setting() -> String {
    spellings(AuthnConfig::JWKS_URI)
}

/// The accepted algorithms' setting, every way it can be given.
pub(crate) fn algorithms_setting() -> String {
    spellings(AuthnConfig::ALGORITHMS)
}

/// The JWK Set endpoint's TLS settings, every way they can be given.
pub(crate) fn tls_setting() -> String {
    format!(
        "{}, {} and {}, or `{}` in an AuthnConfig or JwtOptions built in code",
        nest_rs_config::spellings(AuthnConfig::NAMESPACE, "TLS_CA_CERT"),
        nest_rs_config::spellings(AuthnConfig::NAMESPACE, "TLS_CERT"),
        nest_rs_config::spellings(AuthnConfig::NAMESPACE, "TLS_KEY"),
        bare(AuthnConfig::TLS),
    )
}

/// The key a secret, a private key and a public key make, or why they make
/// none — every combination named, the order of the arms deciding nothing.
fn static_key(
    secret: Option<Secret<String>>,
    private: Option<Secret<String>>,
    public: Option<String>,
) -> Result<JwtKey, AuthError> {
    match (secret, private, public) {
        (Some(secret), None, None) => Ok(JwtKey::Hmac(secret)),
        (Some(_), private @ Some(_), public) | (Some(_), private @ None, public @ Some(_)) => {
            let keys = [
                private
                    .as_ref()
                    .map(|_| spellings(AuthnConfig::PRIVATE_KEY)),
                public.as_ref().map(|_| spellings(AuthnConfig::PUBLIC_KEY)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", and ");
            Err(AuthError::failed(format!(
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
        (None, Some(_), None) => Err(AuthError::failed(format!(
            "{}, is set without {}",
            spellings(AuthnConfig::PRIVATE_KEY),
            spellings(AuthnConfig::PUBLIC_KEY),
        ))),
        (None, None, None) => Err(AuthError::failed(format!(
            "no JWT key configured: set {} for HS256, {} for EdDSA, or {} for an external \
             issuer's keys — or the same field in an AuthnConfig or JwtOptions built in code",
            AuthnConfig::SECRET.spellings(AuthnConfig::NAMESPACE),
            AuthnConfig::PUBLIC_KEY.spellings(AuthnConfig::NAMESPACE),
            AuthnConfig::JWKS_URI.spellings(AuthnConfig::NAMESPACE),
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
                        .map(|_| spellings(AuthnConfig::PRIVATE_KEY)),
                    self.public_key
                        .as_ref()
                        .map(|_| spellings(AuthnConfig::PUBLIC_KEY)),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                if !beside.is_empty() {
                    return Err(AuthError::failed(format!(
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
                if !self.tls.is_empty() {
                    return Err(AuthError::failed(format!(
                        "TLS material — {} — is set without {}: it is what the JWK Set \
                         endpoint's connection trusts and presents, and no JWK Set is fetched",
                        tls_setting(),
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
        options.leeway = self.leeway;
        options.audience = self.audience;
        options.issuer = self.issuer;
        options.allow_any_audience = self.allow_any_audience;
        options.explicit_typing = self.explicit_typing;
        options.expires_in = self.expires_in;
        Ok(options)
    }
}

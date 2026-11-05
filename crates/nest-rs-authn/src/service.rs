//! [`JwtService`] — sign and verify JSON Web Tokens.

use std::time::Duration;

use jsonwebtoken::{
    DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode, errors::ErrorKind,
    get_current_timestamp,
};
use nest_rs_config::{Namespaced, endpoint_shown};
use nest_rs_core::Secret;
use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::Algorithm;
use crate::algorithm::{Family, algorithms_of};
use crate::error::AuthError;
use crate::jwks::Jwks;
use nest_rs_config::ClientTls;

use crate::AuthnConfig;
use crate::binding::ResourceBinding;

/// Prove an EdDSA private key and public key are one pair: a signature the
/// private key makes has to verify under the public key.
fn prove_one_pair(
    encoding: &EncodingKey,
    decoding: &DecodingKey,
    algorithm: Algorithm,
) -> Result<(), AuthError> {
    const PROBE: &[u8] = b"nest-rs-authn: are these two keys one pair";
    let signature = jsonwebtoken::crypto::sign(PROBE, encoding, algorithm.jwt())
        .map_err(|e| AuthError::failed_because("invalid JWT private key", e))?;
    let paired = jsonwebtoken::crypto::verify(&signature, PROBE, decoding, algorithm.jwt())
        .map_err(|e| AuthError::failed_because("invalid JWT public key", e))?;
    if paired {
        return Ok(());
    }
    Err(AuthError::failed(format!(
        "{}, and {}, are not one pair: tokens this service signed would be refused, and tokens \
         the other pair's private key signed accepted",
        crate::config::spellings(AuthnConfig::PRIVATE_KEY),
        crate::config::spellings(AuthnConfig::PUBLIC_KEY),
    )))
}

/// Minimum HS256 shared-secret length: 256 bits (32 bytes). Enforced in
/// [`JwtService::new`], which every constructor path reaches.
pub(crate) const HS256_MIN_SECRET_BYTES: usize = 32;

/// The shortest shared secret `algorithm` accepts: the size of its hash output,
/// RFC 7518 §3.2's floor — a longer hash signed with a shorter key is no stronger
/// than the key.
fn min_hmac_secret_bytes(algorithm: Algorithm) -> usize {
    match algorithm {
        Algorithm::HS384 => 48,
        Algorithm::HS512 => 64,
        _ => HS256_MIN_SECRET_BYTES,
    }
}

/// The families a JWK Set's keys may verify, and a [`JwtKey::Jwks`]'s default:
/// every asymmetric one, never HMAC — a JWK Set is public, and an HMAC
/// algorithm would verify with a key anyone reads.
pub(crate) const JWKS_FAMILIES: &[Family] = &[Family::Rsa, Family::Ec, Family::Ed];

/// `algorithms` as a sentence names them: `HS256, HS384 or HS512`.
pub(crate) fn one_of(algorithms: impl IntoIterator<Item = Algorithm>) -> String {
    let names: Vec<&str> = algorithms.into_iter().map(Algorithm::as_str).collect();
    match names.split_last() {
        Some((last, [])) => (*last).to_owned(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// Prefix of every media type this framework mints for a non-access purpose.
///
/// Reserved so [`JwtService::verify`] refuses a handshake token without also
/// refusing the legacy `typ: JWT` that `explicit_typing = false` accepts.
const HANDSHAKE_TYP_PREFIX: &str = "nrs-";

/// Build the media type for a handshake `purpose` — `"oauth-tx"` ⇒
/// `"nrs-oauth-tx+jwt"`.
fn handshake_typ(purpose: &str) -> String {
    format!("{HANDSHAKE_TYP_PREFIX}{purpose}+jwt")
}

/// Whether `typ` names a purpose this framework reserves, i.e. anything but an
/// access token. Case-insensitive per RFC 9110 §8.3.1, like every other media
/// type comparison here.
fn is_reserved_handshake_typ(typ: &str) -> bool {
    typ.len() > HANDSHAKE_TYP_PREFIX.len()
        && typ[..HANDSHAKE_TYP_PREFIX.len()].eq_ignore_ascii_case(HANDSHAKE_TYP_PREFIX)
}

/// Whose lifetime a token's `exp` states.
#[derive(Clone, Copy)]
enum Lifetime {
    /// The signer's `expires_in` after `iat`, unless the claims name their own:
    /// an access token.
    Signer,
    /// The claims' own, which they must carry: a handshake never inherits the
    /// access-token lifetime.
    Claims,
}

/// The media type RFC 9068 §2.1 assigns to a JWT access token. `at+jwt` is the
/// SHOULD-form; §4 also obliges a verifier to accept the long form, so both are
/// spelled here and nowhere else.
const AT_JWT: &str = "at+jwt";
/// The `application/`-prefixed spelling §4 names beside it.
const AT_JWT_LONG: &str = "application/at+jwt";

/// Key material backing a [`JwtService`].
///
/// `Debug` shows a JWK Set's `uri` through [`endpoint_shown`]: its userinfo or
/// query may carry the issuer's credential.
#[derive(Clone)]
#[non_exhaustive]
pub enum JwtKey {
    /// Shared secret: the same key signs and verifies. Every verifier can also mint.
    Hmac(Secret<String>),
    /// Asymmetric PEM keys. `private_pem` is `None` on a verify-only resource server.
    Pem {
        /// EdDSA private key, PEM. `None` on a verify-only resource server —
        /// then [`sign`](JwtService::sign) refuses.
        private_pem: Option<Secret<String>>,
        /// EdDSA public key, PEM. Always present — verification needs it.
        public_pem: String,
    },
    /// The JWK Set (RFC 7517 §5) an external issuer publishes — Keycloak,
    /// Auth0, better-auth — fetched when a token first needs it and kept
    /// fresh. Verify-only: [`sign`](JwtService::sign) and the handshake
    /// methods refuse.
    Jwks {
        /// Where the set is fetched; `https` only.
        uri: String,
        /// What the endpoint's certificate must chain to — the system's
        /// authorities by default — and the certificate presented to it.
        tls: ClientTls,
    },
}

impl std::fmt::Debug for JwtKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hmac(secret) => f.debug_tuple("Hmac").field(secret).finish(),
            Self::Pem {
                private_pem,
                public_pem,
            } => f
                .debug_struct("Pem")
                .field("private_pem", private_pem)
                .field("public_pem", public_pem)
                .finish(),
            Self::Jwks { uri, tls } => f
                .debug_struct("Jwks")
                .field("uri", &endpoint_shown(uri))
                .field("tls", tls)
                .finish(),
        }
    }
}

/// Runtime JWT settings passed to [`JwtService::new`]; a field added in a minor
/// release is set by assignment after a constructor, never in a literal.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct JwtOptions {
    /// The key material — an HMAC secret, an EdDSA PEM pair, or an issuer's
    /// JWK Set — backing sign/verify.
    pub key: JwtKey,
    /// The algorithms a token may be signed with. A secret or a PEM key signs
    /// and verifies with exactly one, which must fit it; a JWK Set verifies
    /// any it holds, each key only with an algorithm of its own type, and never
    /// an `HS*` one. Narrowing it binds a key that names no `alg` to the one
    /// algorithm its issuer signs with (RFC 8725 §3.1).
    pub algorithms: Vec<Algorithm>,
    /// The lifetime the signer stamps as a minted token's `exp`, after its
    /// `iat`, unless its claims name their own (default 1 hour).
    pub expires_in: Duration,
    /// Clock skew tolerated when validating `exp` / `nbf`.
    pub leeway: Duration,
    /// When set, tokens must carry a matching `aud` claim, and a token this
    /// service mints for itself carries it.
    ///
    /// Leaving it unset does **not** switch the audience check off — see
    /// [`allow_any_audience`](Self::allow_any_audience). A token minted for
    /// another resource names that resource instead
    /// ([`JwtService::sign_for`]).
    pub audience: Option<String>,
    /// When set, tokens must carry a matching `iss` claim, and every token this
    /// service mints carries it.
    pub issuer: Option<String>,
    /// RFC 9068 explicit typing: stamp `typ: at+jwt` when minting, and refuse a
    /// token whose `typ` is anything else when verifying. `true`, because §4
    /// states the verifier's half as a MUST and §2.1 gives the reason — an
    /// OpenID Connect ID Token must not be accepted as an access token.
    ///
    /// Set it `false` only to verify tokens from an issuer that predates the
    /// profile and mints a plain `typ: JWT`, or none.
    pub explicit_typing: bool,
    /// Opt out of RFC 7519 §4.1.3 — accept a token whose `aud` names a
    /// principal this service is not. `false`: without it, every app sharing an
    /// issuer is a deputy for every other.
    ///
    /// Turn it on only for a deliberately audience-agnostic verifier (a
    /// debugging proxy, an introspection endpoint); [`JwtService::new`] refuses
    /// it beside [`audience`](Self::audience).
    pub allow_any_audience: bool,
}

impl JwtOptions {
    /// The clock-skew leeway a verifier allows unless told otherwise.
    pub(crate) const DEFAULT_LEEWAY: Duration = Duration::from_secs(30);

    /// The lifetime of a token minted unless told otherwise: an hour.
    pub(crate) const DEFAULT_EXPIRES_IN: Duration = Duration::from_secs(3600);

    /// Options over `key`, accepting its default algorithms — HS256 for a
    /// secret, EdDSA for a PEM key, every asymmetric one for a JWK Set — every
    /// other field at its default.
    pub(crate) fn with_key(key: JwtKey) -> Self {
        let algorithms = match &key {
            JwtKey::Hmac(_) => vec![Algorithm::HS256],
            JwtKey::Pem { .. } => vec![Algorithm::EdDSA],
            JwtKey::Jwks { .. } => algorithms_of(JWKS_FAMILIES).collect(),
        };
        Self {
            key,
            algorithms,
            expires_in: Self::DEFAULT_EXPIRES_IN,
            leeway: Self::DEFAULT_LEEWAY,
            audience: None,
            issuer: None,
            explicit_typing: true,
            allow_any_audience: false,
        }
    }

    /// HS256 options from a shared secret. Audience/issuer are unset (no
    /// claim check) and TTL defaults to 1 hour — layer on via the fields.
    pub fn new(secret: impl Into<String>) -> Self {
        Self::with_key(JwtKey::Hmac(Secret::new(secret.into())))
    }

    /// EdDSA options with both keys — a token *issuer* that can sign and verify.
    pub fn eddsa(private_pem: impl Into<String>, public_pem: impl Into<String>) -> Self {
        Self::with_key(JwtKey::Pem {
            private_pem: Some(Secret::new(private_pem.into())),
            public_pem: public_pem.into(),
        })
    }

    /// EdDSA options with only the public key — a *resource server* that can
    /// verify but never mint (the `apps/api` posture).
    pub fn eddsa_verify(public_pem: impl Into<String>) -> Self {
        Self::with_key(JwtKey::Pem {
            private_pem: None,
            public_pem: public_pem.into(),
        })
    }

    /// Options verifying the tokens of an external issuer against the JWK Set
    /// it publishes at `uri` — a resource server that never mints. Every
    /// asymmetric algorithm is accepted until [`algorithms`](Self::algorithms)
    /// narrows it, and the endpoint's certificate chains to the system's
    /// authorities.
    pub fn jwks(uri: impl Into<String>) -> Self {
        Self::with_key(JwtKey::Jwks {
            uri: uri.into(),
            tls: ClientTls::default(),
        })
    }
}

nest_rs_core::inventory::submit! {
    nest_rs_core::DependencyRemedy::new(
        std::any::TypeId::of::<JwtService>,
        "Import `nest_rs::authn::AuthnModule`, which builds the one `JwtService` every guard and \
         strategy verifies with.",
    )
}

/// Singleton token signer/verifier, built once at boot and injected wherever a
/// token is signed or verified.
pub struct JwtService {
    /// `None` on a verify-only service.
    signer: Option<Signer>,
    verifier: Verifier,
    /// Whether RFC 9068 explicit typing is enforced on the verifying side.
    /// Read by [`verify`](Self::verify); the signing side already carries it in
    /// the signer's header.
    explicit_typing: bool,
    expires_in: Duration,
    /// Stamped onto a token minted for this service itself when configured —
    /// the validation requires it on the verifying side, so the signer must not
    /// leave it to the app's claims struct. A token minted for another resource
    /// names that resource instead.
    audience: Option<String>,
    /// Stamped onto every minted token when configured, beside `iat` and `jti`.
    issuer: Option<String>,
}

struct Signer {
    key: EncodingKey,
    header: Header,
}

#[expect(
    clippy::large_enum_variant,
    reason = "one per process, built at boot: boxing the larger variant saves nothing"
)]
enum Verifier {
    /// One key, one algorithm.
    Static {
        key: DecodingKey,
        validation: Validation,
        /// What a handshake meets: the audience and issuer this service stamps
        /// on its own, which a resource binding never widens.
        handshake: Validation,
    },
    /// An issuer's JWK Set, a validation per accepted algorithm: the token's
    /// `alg` picks among them, and only among them.
    Jwks {
        set: Jwks,
        /// One per algorithm: jsonwebtoken 11 refuses every token under a
        /// `Validation` whose algorithms span two key families.
        validations: Vec<(Algorithm, Validation)>,
    },
}

/// `algorithms` without repeats, once each fits `key`: a static key takes
/// exactly one algorithm of its type, a JWK Set any asymmetric ones.
fn accepted(key: &JwtKey, algorithms: &[Algorithm]) -> Result<Vec<Algorithm>, AuthError> {
    let mut accepted: Vec<Algorithm> = Vec::with_capacity(algorithms.len());
    for algorithm in algorithms {
        if !accepted.contains(algorithm) {
            accepted.push(*algorithm);
        }
    }
    let (families, key_name): (&[Family], &str) = match key {
        JwtKey::Hmac(_) => (&[Family::Hmac], "an HMAC secret"),
        JwtKey::Pem { .. } => (&[Family::Ed], "an EdDSA key"),
        JwtKey::Jwks { .. } => (JWKS_FAMILIES, "a JWK Set, whose keys anyone can read"),
    };
    let fitting = || one_of(algorithms_of(families));
    if accepted.is_empty() {
        return Err(AuthError::failed(format!(
            "{}, names no algorithm: name {}",
            crate::config::algorithms_setting(),
            fitting()
        )));
    }
    if let Some(misfit) = accepted
        .iter()
        .find(|algorithm| !families.contains(&algorithm.family()))
    {
        return Err(AuthError::failed(format!(
            "the {misfit} algorithm cannot be used with {key_name}: use {}",
            fitting()
        )));
    }
    if !matches!(key, JwtKey::Jwks { .. }) && accepted.len() > 1 {
        return Err(AuthError::failed(format!(
            "{}, names {} algorithms, and {key_name} signs and verifies with one",
            crate::config::algorithms_setting(),
            accepted.len()
        )));
    }
    Ok(accepted)
}

impl JwtService {
    /// Build the service from [`JwtOptions`], deriving keys and pinning the
    /// validation policy: `exp`/`nbf` always checked; `aud` always checked per
    /// RFC 7519 §4.1.3 — a token carrying an audience this service is not named
    /// in is refused whether or not one is configured — and `aud`/`iss`
    /// additionally *required-present* when configured, so an omitting token
    /// fails closed. Errors on unparseable PEM key material, on an HMAC secret
    /// shorter than its hash, on an algorithm that does not fit the key, on a
    /// JWK Set URI that is not `https`, and on the
    /// [`allow_any_audience`](JwtOptions::allow_any_audience) contradiction.
    ///
    /// A JWK Set is not fetched here: the first token that needs it fetches it.
    pub fn new(options: JwtOptions) -> Result<Self, AuthError> {
        let issuer = options.issuer.clone();
        Self::build(options, issuer.as_slice())
    }

    /// [`new`](Self::new) for the resource `binding` describes: its audience
    /// replaces the configured one, and a token's `iss` must be one of its
    /// issuers (RFC 9068 §4). A token this service mints for itself keeps the
    /// configured issuer.
    pub(crate) fn bound(
        mut options: JwtOptions,
        binding: &ResourceBinding,
    ) -> Result<Self, AuthError> {
        if binding.issuers.is_empty() {
            return Err(AuthError::failed(
                "the resource this service verifies tokens for trusts no authorization server, \
                 so no token's issuer could be checked",
            ));
        }
        options.audience = Some(binding.audience.clone());
        Self::build(options, &binding.issuers)
    }

    /// The service over `options`, a token's `iss` one of `issuers` when any.
    fn build(options: JwtOptions, issuers: &[String]) -> Result<Self, AuthError> {
        // The ranges the variables are held to, held again here: a `JwtOptions`
        // built in code reaches this constructor without a config read, and an
        // unbounded lifetime or leeway overflows the arithmetic below it.
        for (setting, value) in [
            (
                AuthnConfig::EXPIRES_IN.pinned_through("JwtOptions::expires_in"),
                options.expires_in,
            ),
            (
                AuthnConfig::LEEWAY.pinned_through("JwtOptions::leeway"),
                options.leeway,
            ),
        ] {
            setting
                .check(AuthnConfig::NAMESPACE, value)
                .map_err(|refused| {
                    AuthError::failed_because("the JWT options are refused", refused)
                })?;
        }
        // A key and an algorithm that cannot work together fail here, not at the
        // first request.
        let algorithms = accepted(&options.key, &options.algorithms)?;
        let signing = algorithms[0];

        // A contradiction, refused rather than resolved.
        if options.allow_any_audience {
            if options.audience.is_some() {
                return Err(AuthError::failed(format!(
                    "{} contradicts {}: an audience-agnostic verifier cannot also require an audience",
                    AuthnConfig::ALLOW_ANY_AUDIENCE.var(AuthnConfig::NAMESPACE),
                    AuthnConfig::AUDIENCE.var(AuthnConfig::NAMESPACE),
                )));
            }
            // Not silent: named once per boot, so the opt-out is actionable.
            tracing::warn!(
                target: crate::TARGET,
                var = %AuthnConfig::ALLOW_ANY_AUDIENCE.var(AuthnConfig::NAMESPACE),
                "audience validation is disabled — a token minted for another service verifies here",
            );
        }

        // RFC 9068 §2.1 and §4 (explicit typing, RFC 8725 §3.11): mint `at+jwt`,
        // and `verify` refuses any other `typ`.
        let mut header = Header::new(signing.jwt());
        if options.explicit_typing {
            header.typ = Some(AT_JWT.to_owned());
        }

        let (signer, verifier) = match &options.key {
            JwtKey::Hmac(secret) => {
                let secret = secret.expose_secret();
                // Fail closed at the derivation point every constructor reaches.
                if secret.trim().is_empty() {
                    return Err(AuthError::failed(format!(
                        "{}, must not be empty",
                        crate::config::secret_setting()
                    )));
                }
                // RFC 7518 §3.2: a key of the same size as the hash output, or
                // larger — 256 bits for HS256, 384 for HS384, 512 for HS512.
                let minimum = min_hmac_secret_bytes(signing);
                if secret.len() < minimum {
                    return Err(AuthError::failed(format!(
                        "{}, must be at least {minimum} bytes ({} bits) for {signing}; got {}",
                        crate::config::secret_setting(),
                        minimum * 8,
                        secret.len()
                    )));
                }
                let bytes = secret.as_bytes();
                (
                    Some(Signer {
                        key: EncodingKey::from_secret(bytes),
                        header,
                    }),
                    Verifier::Static {
                        key: DecodingKey::from_secret(bytes),
                        validation: validation(&options, signing, issuers),
                        handshake: validation(&options, signing, options.issuer.as_slice()),
                    },
                )
            }
            JwtKey::Pem {
                private_pem,
                public_pem,
            } => {
                let decoding = DecodingKey::from_ed_pem(public_pem.as_bytes()).map_err(|e| {
                    AuthError::failed_because(
                        format!(
                            "{}, is not an EdDSA public key in PEM form",
                            crate::config::spellings(AuthnConfig::PUBLIC_KEY),
                        ),
                        e,
                    )
                })?;
                let signer = match private_pem {
                    Some(pem) => {
                        let encoding = EncodingKey::from_ed_pem(pem.expose_secret().as_bytes())
                            .map_err(|e| {
                                AuthError::failed_because(
                                    format!(
                                        "{}, is not an EdDSA private key in PEM form",
                                        crate::config::spellings(AuthnConfig::PRIVATE_KEY),
                                    ),
                                    e,
                                )
                            })?;
                        prove_one_pair(&encoding, &decoding, signing)?;
                        Some(Signer {
                            key: encoding,
                            header,
                        })
                    }
                    None => None,
                };
                (
                    signer,
                    Verifier::Static {
                        key: decoding,
                        validation: validation(&options, signing, issuers),
                        handshake: validation(&options, signing, options.issuer.as_slice()),
                    },
                )
            }
            JwtKey::Jwks { uri, tls } => (
                None,
                Verifier::Jwks {
                    set: Jwks::new(uri, tls, &algorithms)?,
                    validations: algorithms
                        .iter()
                        .map(|algorithm| (*algorithm, validation(&options, *algorithm, issuers)))
                        .collect(),
                },
            ),
        };

        Ok(Self {
            signer,
            verifier,
            explicit_typing: options.explicit_typing,
            expires_in: options.expires_in,
            audience: options.audience,
            issuer: options.issuer,
        })
    }

    /// Sign `claims` into a compact JWT this service mints for itself. Errors
    /// on a verify-only service (no encoding key) or a serialization failure.
    ///
    /// The token carries what the signer owns of RFC 9068 §2.2's required set
    /// — `iat`, `exp` this service's lifetime after it, a fresh `jti`, and the
    /// configured `aud` and `iss` — beside `claims`, which name the rest:
    /// `sub` and `client_id`. A claim `claims` already carry is never
    /// overwritten. A token for another resource is
    /// [`sign_for`](Self::sign_for)'s.
    pub fn sign<C: Serialize>(&self, claims: &C) -> Result<String, AuthError> {
        let signer = self.signer()?;
        self.encode_with(signer, &signer.header, claims, None, Lifetime::Signer)
    }

    /// Sign `claims` into a compact JWT for `resource`, the RFC 8707 §2
    /// resource the token was requested for: its `aud`, whatever audience this
    /// service requires of its own, so only a verifier serving that resource
    /// accepts it.
    ///
    /// The token carries the stamps [`sign`](Self::sign) does but the
    /// audience. Errors on a blank `resource`, on `claims` naming an `aud` of
    /// their own, on a verify-only service, or on a serialization failure.
    /// Which resources a caller may be minted a token for is the issuer's to
    /// decide before it calls this.
    pub fn sign_for<C: Serialize>(&self, resource: &str, claims: &C) -> Result<String, AuthError> {
        if resource.trim().is_empty() {
            return Err(AuthError::failed(
                "a blank resource names no resource to sign a token for",
            ));
        }
        let signer = self.signer()?;
        self.encode_with(
            signer,
            &signer.header,
            claims,
            Some(resource),
            Lifetime::Signer,
        )
    }

    /// The signing half, or the refusal a verify-only service answers.
    fn signer(&self) -> Result<&Signer, AuthError> {
        self.signer.as_ref().ok_or_else(|| {
            AuthError::failed("this JwtService is verify-only — no signing key configured")
        })
    }

    /// Sign `claims` under `header` for `resource` — this service itself when
    /// `None` — stamping the signer's claims first, `exp` as `lifetime` says.
    fn encode_with<C: Serialize>(
        &self,
        signer: &Signer,
        header: &Header,
        claims: &C,
        resource: Option<&str>,
        lifetime: Lifetime,
    ) -> Result<String, AuthError> {
        encode(
            header,
            &self.stamped(claims, resource, lifetime)?,
            &signer.key,
        )
        .map_err(|e| AuthError::failed_because("the claims could not be signed", e))
    }

    /// `claims` as a JSON object carrying `iat`, `exp` the configured lifetime
    /// after it (for a [`Lifetime::Signer`] token), a fresh `jti`, the
    /// configured `iss`, and as `aud` either `resource` or, for a token this
    /// service mints for itself, the configured audience.
    ///
    /// Verification requires `aud` and `iss` when configured, so a claims
    /// struct omitting them would mint tokens its own verifier rejects. A claim
    /// the claims type carries is never overwritten — but an `aud` beside
    /// `resource`, which it would contradict, is refused, and so are
    /// [`Lifetime::Claims`] claims carrying no `exp`; one serialized as `null`
    /// is absent.
    fn stamped<C: Serialize>(
        &self,
        claims: &C,
        resource: Option<&str>,
        lifetime: Lifetime,
    ) -> Result<serde_json::Value, AuthError> {
        let mut value = serde_json::to_value(claims)
            .map_err(|e| AuthError::failed_because("the claims could not be serialized", e))?;
        let Some(map) = value.as_object_mut() else {
            // A non-object body cannot carry registered claims; the encoder fails it.
            return Ok(value);
        };
        let signer_s_lifetime = match lifetime {
            Lifetime::Signer => Some(self.expires_in.as_secs()),
            Lifetime::Claims if map.get("exp").is_none_or(serde_json::Value::is_null) => {
                return Err(AuthError::failed(
                    "a handshake's claims carry no `exp` — name its lifetime with \
                     `JwtService::expiry_in(secs)`: a handshake never inherits the access-token \
                     lifetime",
                ));
            }
            Lifetime::Claims => None,
        };
        let audience = match resource {
            Some(resource) => {
                if map.get("aud").is_some_and(|aud| !aud.is_null()) {
                    return Err(AuthError::failed(
                        "the claims carry an `aud` of their own beside the resource they are \
                         signed for — leave `aud` to the signer",
                    ));
                }
                Some(resource.to_owned())
            }
            None => self.audience.clone(),
        };
        let issued = get_current_timestamp();
        let stamps: [(&str, Option<serde_json::Value>); 5] = [
            ("iat", Some(issued.into())),
            (
                "exp",
                signer_s_lifetime.map(|secs| issued.saturating_add(secs).into()),
            ),
            ("jti", Some(Uuid::new_v4().to_string().into())),
            ("aud", audience.map(Into::into)),
            ("iss", self.issuer.clone().map(Into::into)),
        ];
        for (key, stamp) in stamps {
            if let Some(stamp) = stamp
                && map.get(key).is_none_or(serde_json::Value::is_null)
            {
                map.insert(key.to_owned(), stamp);
            }
        }
        Ok(value)
    }

    /// Verify `token` and deserialize its claims into `C`, applying the pinned
    /// `exp`/`nbf`/`aud`/`iss` validation. Maps the failure to a typed
    /// [`AuthError`] (expired, bad signature, wrong algorithm, …).
    ///
    /// Against a JWK Set it may wait on one fetch of the set, bounded by
    /// [`JWKS_FETCH_TIMEOUT`](crate::JWKS_FETCH_TIMEOUT); a set that cannot be
    /// had is [`AuthError::Unavailable`], never a verdict on the token. Against
    /// a static key it never waits.
    pub async fn verify<C: DeserializeOwned>(&self, token: &str) -> Result<C, AuthError> {
        let data = match &self.verifier {
            Verifier::Static {
                key, validation, ..
            } => decode::<C>(token, key, validation).map_err(map_decode_error)?,
            Verifier::Jwks { set, validations } => {
                // The header is unsigned: it only picks among what the verifier
                // already accepts, and the signature then has to hold.
                let header = decode_header(token).map_err(map_decode_error)?;
                let Some((algorithm, validation)) = Algorithm::from_jwt(header.alg)
                    .and_then(|named| validations.iter().find(|(accepted, _)| *accepted == named))
                else {
                    return Err(AuthError::InvalidAlgorithm);
                };
                let key = set.key_for(header.kid.as_deref(), *algorithm).await?;
                decode::<C>(token, &key, validation).map_err(map_decode_error)?
            }
        };
        // RFC 9068 §4, case-insensitive (RFC 9110 §8.3.1), checked after the
        // signature so an unsigned header never steers it. `explicit_typing` off
        // relaxes the `at+jwt` check, never the reserved handshake namespace.
        let typ = data.header.typ.as_deref();
        if typ.is_some_and(is_reserved_handshake_typ) {
            return Err(AuthError::InvalidToken);
        }
        if self.explicit_typing {
            let typ = typ.unwrap_or_default();
            if !typ.eq_ignore_ascii_case(AT_JWT) && !typ.eq_ignore_ascii_case(AT_JWT_LONG) {
                return Err(AuthError::InvalidToken);
            }
        }
        Ok(data.claims)
    }

    /// Sign a short-lived **handshake** token — a value this deployment mints
    /// for one of its own flows (an OAuth transaction cookie), never a
    /// credential a resource server should accept.
    ///
    /// `typ` names the purpose (RFC 8725 §3.11): [`verify`](Self::verify)
    /// refuses this token whatever `explicit_typing` is, and
    /// [`verify_handshake`](Self::verify_handshake) refuses an access token.
    /// `purpose` names the flow (`"oauth-tx"`); the reserved media type is built
    /// from it.
    ///
    /// The token carries the stamps [`sign`](Self::sign) does but `exp`, which
    /// `claims` name themselves with [`expiry_in`](Self::expiry_in): claims
    /// carrying none are refused rather than given the access-token lifetime.
    pub fn sign_handshake<C: Serialize>(
        &self,
        purpose: &str,
        claims: &C,
    ) -> Result<String, AuthError> {
        let signer = self.signer()?;
        let mut header = signer.header.clone();
        header.typ = Some(handshake_typ(purpose));
        self.encode_with(signer, &header, claims, None, Lifetime::Claims)
    }

    /// Verify a handshake token minted by [`sign_handshake`](Self::sign_handshake)
    /// for the same `purpose`.
    ///
    /// The type check is unconditional — a flow's separation from access tokens
    /// cannot depend on a configuration flag — and it is exact, so a
    /// transaction minted for one flow does not verify on another's.
    ///
    /// A handshake verifies only under the static key that signs this
    /// deployment's own: a service verifying an issuer's JWK Set mints none, and
    /// refuses as [`sign_handshake`](Self::sign_handshake) does.
    pub fn verify_handshake<C: DeserializeOwned>(
        &self,
        purpose: &str,
        token: &str,
    ) -> Result<C, AuthError> {
        let Verifier::Static { key, handshake, .. } = &self.verifier else {
            return Err(AuthError::failed(
                "this JwtService verifies an issuer's JWK Set — it verifies no handshake of \
                 this deployment's own",
            ));
        };
        let data = decode::<C>(token, key, handshake).map_err(map_decode_error)?;
        let found = data.header.typ.as_deref().unwrap_or_default();
        if !found.eq_ignore_ascii_case(&handshake_typ(purpose)) {
            return Err(AuthError::InvalidToken);
        }
        Ok(data.claims)
    }

    /// Absolute `exp` for a token that should live exactly `secs` seconds —
    /// a short-lived handshake token (the OAuth transaction), whose claims must
    /// name it: [`sign_handshake`](Self::sign_handshake) stamps no lifetime.
    ///
    /// Saturates rather than wrapping: a wrapped sum would mint a token already expired.
    pub fn expiry_in(&self, secs: u64) -> u64 {
        get_current_timestamp().saturating_add(secs)
    }

    /// The configured default token lifetime, in seconds — e.g. to report a
    /// token's `expires_in` in an OAuth token response.
    pub fn ttl_secs(&self) -> u64 {
        self.expires_in.as_secs()
    }
}

/// The validation every token meets, for one `algorithm`, its `iss` one of
/// `issuers` when any.
fn validation(options: &JwtOptions, algorithm: Algorithm, issuers: &[String]) -> Validation {
    let mut validation = Validation::new(algorithm.jwt());
    // Pinned, not left to a library default a future version could flip.
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.leeway = options.leeway.as_secs();
    // RFC 7519 §4.1.3 is jsonwebtoken's `validate_aud`: on whether or not an
    // audience is configured, so an unconfigured verifier still refuses a
    // token minted for a sibling service by the shared issuer.
    validation.validate_aud = !options.allow_any_audience;
    // `set_audience`/`set_issuer` only compare a claim the token carries;
    // requiring it makes an omitting token fail closed.
    if let Some(aud) = &options.audience {
        validation.set_audience(&[aud.as_str()]);
        validation.required_spec_claims.insert("aud".to_owned());
    }
    if !issuers.is_empty() {
        validation.set_issuer(issuers);
        validation.required_spec_claims.insert("iss".to_owned());
    }
    // No `iss` twin of `validate_aud`: RFC 7519 §4.1.1 states no such clause.
    validation
}

fn map_decode_error(err: jsonwebtoken::errors::Error) -> AuthError {
    let mapped = match err.kind() {
        ErrorKind::ExpiredSignature => AuthError::Expired,
        ErrorKind::InvalidSignature => AuthError::InvalidSignature,
        ErrorKind::InvalidAlgorithm => AuthError::InvalidAlgorithm,
        ErrorKind::ImmatureSignature => AuthError::NotYetValid,
        _ => AuthError::InvalidToken,
    };
    // `AuthnGuard` files the one `warn` per failure; this stays `debug`.
    if !matches!(mapped, AuthError::Expired) {
        // A claim that does not decode is said without the value serde quotes.
        let error = match err.kind() {
            ErrorKind::Json(json) => format!(
                "the token's JSON does not decode: {}",
                nest_rs_core::DecodeError::new(json)
            ),
            _ => nest_rs_core::error_message(&err),
        };
        tracing::debug!(target: crate::TARGET, error = %error, "JWT verification failed");
    }
    mapped
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    fn refusal(options: JwtOptions) -> String {
        match JwtService::new(options) {
            Err(refused @ AuthError::Failed { .. }) => nest_rs_core::error_message(&refused),
            Err(other) => panic!("refused as {other:?}"),
            Ok(_) => panic!("built"),
        }
    }

    /// RFC 8725 §3.1: what a JWK Set verifies is the verifier's, and no HMAC
    /// algorithm is among it — the set is public.
    #[test]
    fn a_jwk_set_never_accepts_an_hmac_algorithm() {
        for hmac in [Algorithm::HS256, Algorithm::HS384, Algorithm::HS512] {
            let mut options = JwtOptions::jwks("https://issuer.example/jwks");
            options.algorithms = vec![Algorithm::RS256, hmac];
            let refused = refusal(options);
            assert!(
                refused.contains("algorithm cannot be used with a JWK Set"),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_jwk_set_accepts_every_asymmetric_algorithm_by_default() {
        let options = JwtOptions::jwks("https://issuer.example/jwks");
        assert_eq!(options.algorithms.len(), 9, "{:?}", options.algorithms);
        assert!(
            options
                .algorithms
                .iter()
                .all(|algorithm| algorithm.family() != Family::Hmac)
        );
        JwtService::new(options).expect("the default set fits a JWK Set");
    }

    #[test]
    fn a_list_of_algorithms_reads_as_a_sentence() {
        assert_eq!(
            one_of(algorithms_of(&[Family::Hmac])),
            "HS256, HS384 or HS512"
        );
        assert_eq!(one_of(algorithms_of(&[Family::Ed])), "EdDSA");
    }

    #[test]
    fn a_static_key_signs_with_one_algorithm() {
        let mut options = JwtOptions::new(SECRET.repeat(2));
        options.algorithms = vec![Algorithm::HS256, Algorithm::HS512];
        let refused = refusal(options);
        assert!(
            refused.contains("names 2 algorithms") && refused.contains("ALGORITHMS"),
            "{refused}"
        );

        let mut repeated = JwtOptions::new(SECRET);
        repeated.algorithms = vec![Algorithm::HS256, Algorithm::HS256];
        JwtService::new(repeated).expect("a repeat is one algorithm");
    }

    #[test]
    fn no_algorithm_at_all_is_refused() {
        let mut options = JwtOptions::jwks("https://issuer.example/jwks");
        options.algorithms.clear();
        assert!(refusal(options).contains("names no algorithm"));
    }

    /// The service a JWK Set backs mints nothing — handshakes included.
    #[test]
    fn a_jwk_set_service_signs_nothing() {
        let jwt = JwtService::new(JwtOptions::jwks("https://issuer.example/jwks")).expect("built");
        assert!(matches!(
            jwt.sign(&serde_json::json!({})),
            Err(AuthError::Failed { .. })
        ));
        assert!(matches!(
            jwt.sign_handshake("oauth-tx", &serde_json::json!({})),
            Err(AuthError::Failed { .. })
        ));
        assert!(matches!(
            jwt.verify_handshake::<serde_json::Value>("oauth-tx", "a.b.c"),
            Err(AuthError::Failed { .. })
        ));
    }
}

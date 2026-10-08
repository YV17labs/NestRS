//! [`JwtService`] — sign and verify JSON Web Tokens.

use std::time::Duration;

use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode, errors::ErrorKind,
    get_current_timestamp,
};
use nest_rs_config::{Namespaced, var_name};
use serde::{Serialize, de::DeserializeOwned};

use crate::AuthnConfig;
use crate::error::AuthError;

/// Prove an EdDSA private key and public key are one pair: a signature the
/// private key makes has to verify under the public key.
fn prove_one_pair(
    encoding: &EncodingKey,
    decoding: &DecodingKey,
    algorithm: Algorithm,
) -> Result<(), AuthError> {
    const PROBE: &[u8] = b"nest-rs-authn: are these two keys one pair";
    let signature = jsonwebtoken::crypto::sign(PROBE, encoding, algorithm)
        .map_err(|e| AuthError::Failed(format!("invalid JWT private key: {e}")))?;
    let paired = jsonwebtoken::crypto::verify(&signature, PROBE, decoding, algorithm)
        .map_err(|e| AuthError::Failed(format!("invalid JWT public key: {e}")))?;
    if paired {
        return Ok(());
    }
    Err(AuthError::Failed(format!(
        "{}, and {}, are not one pair: tokens this service signed would be refused, and tokens \
         the other pair's private key signed accepted",
        crate::config::spellings("PRIVATE_KEY", "private_key"),
        crate::config::spellings("PUBLIC_KEY", "public_key"),
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

/// The media type RFC 9068 §2.1 assigns to a JWT access token. `at+jwt` is the
/// SHOULD-form; §4 also obliges a verifier to accept the long form, so both are
/// spelled here and nowhere else.
const AT_JWT: &str = "at+jwt";
/// The `application/`-prefixed spelling §4 names beside it.
const AT_JWT_LONG: &str = "application/at+jwt";

/// Key material backing a [`JwtService`].
#[derive(Clone)]
pub enum JwtKey {
    /// Shared secret: the same key signs and verifies. Every verifier can also mint.
    Hmac(String),
    /// Asymmetric PEM keys. `private_pem` is `None` on a verify-only resource server.
    Pem {
        /// EdDSA private key, PEM. `None` on a verify-only resource server —
        /// then [`sign`](JwtService::sign) refuses.
        private_pem: Option<String>,
        /// EdDSA public key, PEM. Always present — verification needs it.
        public_pem: String,
    },
}

/// Runtime JWT settings passed to [`AuthnModule::for_root`](crate::AuthnModule::for_root).
#[derive(Clone)]
pub struct JwtOptions {
    /// The key material (HMAC secret or EdDSA PEM pair) backing sign/verify.
    pub key: JwtKey,
    /// The signing/verifying algorithm; kept alongside [`key`](Self::key) so
    /// the header and validation agree on it.
    pub algorithm: Algorithm,
    /// Lifetime applied to minted tokens' `exp` (default 1 hour).
    pub expires_in: Duration,
    /// Clock skew tolerated when validating `exp` / `nbf`.
    pub leeway: Duration,
    /// When set, tokens must carry a matching `aud` claim.
    ///
    /// Leaving it unset does **not** switch the audience check off — see
    /// [`allow_any_audience`](Self::allow_any_audience).
    pub audience: Option<String>,
    /// When set, tokens must carry a matching `iss` claim.
    pub issuer: Option<String>,
    /// RFC 9068 explicit typing: stamp `typ: at+jwt` when minting, and refuse a
    /// token whose `typ` is anything else when verifying. `true`, because §4
    /// states the verifier's half as a MUST and §2.1 gives the reason — an
    /// OpenID Connect ID Token must not be accepted as an access token.
    ///
    /// Set it `false` only to verify tokens from an issuer that predates the
    /// profile and mints a plain `typ: JWT`.
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
    const DEFAULT_LEEWAY: Duration = Duration::from_secs(30);

    /// HS256 options from a shared secret. Audience/issuer are unset (no
    /// claim check) and TTL defaults to 1 hour — layer on via the fields.
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            key: JwtKey::Hmac(secret.into()),
            algorithm: Algorithm::HS256,
            expires_in: Duration::from_secs(3600),
            leeway: Self::DEFAULT_LEEWAY,
            audience: None,
            issuer: None,
            explicit_typing: true,
            allow_any_audience: false,
        }
    }

    /// EdDSA options with both keys — a token *issuer* that can sign and verify.
    pub fn eddsa(private_pem: impl Into<String>, public_pem: impl Into<String>) -> Self {
        Self {
            key: JwtKey::Pem {
                private_pem: Some(private_pem.into()),
                public_pem: public_pem.into(),
            },
            algorithm: Algorithm::EdDSA,
            expires_in: Duration::from_secs(3600),
            leeway: Self::DEFAULT_LEEWAY,
            audience: None,
            issuer: None,
            explicit_typing: true,
            allow_any_audience: false,
        }
    }

    /// EdDSA options with only the public key — a *resource server* that can
    /// verify but never mint (the `apps/api` posture).
    pub fn eddsa_verify(public_pem: impl Into<String>) -> Self {
        Self {
            key: JwtKey::Pem {
                private_pem: None,
                public_pem: public_pem.into(),
            },
            algorithm: Algorithm::EdDSA,
            expires_in: Duration::from_secs(3600),
            leeway: Self::DEFAULT_LEEWAY,
            audience: None,
            issuer: None,
            explicit_typing: true,
            allow_any_audience: false,
        }
    }
}

/// Singleton token signer/verifier, built once at boot and injected wherever a
/// token is signed or verified. `encoding` is `None` on a verify-only server.
pub struct JwtService {
    encoding: Option<EncodingKey>,
    decoding: DecodingKey,
    header: Header,
    validation: Validation,
    /// Whether RFC 9068 explicit typing is enforced on the verifying side.
    /// Read by [`verify`](Self::verify); the signing side already carries it in
    /// `header`.
    explicit_typing: bool,
    expires_in: Duration,
    /// Stamped onto every minted token when configured — `validation` requires
    /// them on the verifying side, so the signer must not leave them to the
    /// app's claims struct.
    audience: Option<String>,
    issuer: Option<String>,
}

impl JwtService {
    /// Build the service from [`JwtOptions`], deriving keys and pinning the
    /// validation policy: `exp`/`nbf` always checked; `aud` always checked per
    /// RFC 7519 §4.1.3 — a token carrying an audience this service is not named
    /// in is refused whether or not one is configured — and `aud`/`iss`
    /// additionally *required-present* when configured, so an omitting token
    /// fails closed. Errors on unparseable PEM key material, on an HS256 secret
    /// under 256 bits, and on the
    /// [`allow_any_audience`](JwtOptions::allow_any_audience) contradiction.
    pub fn new(options: JwtOptions) -> Result<Self, AuthError> {
        // The ranges the variables are held to, held again here: a `JwtOptions`
        // built in code reaches this constructor without a config read, and an
        // unbounded lifetime or leeway overflows the arithmetic below it.
        for (bounds, field, value) in [
            (
                crate::config::EXPIRES_IN,
                "JwtOptions::expires_in",
                options.expires_in,
            ),
            (crate::config::LEEWAY, "JwtOptions::leeway", options.leeway),
        ] {
            bounds
                .check(AuthnConfig::NAMESPACE, field, value)
                .map_err(|refused| AuthError::Failed(refused.to_string()))?;
        }
        // A key and an algorithm that cannot work together fail here, not at the
        // first request.
        let fits = match &options.key {
            JwtKey::Hmac(_) => matches!(
                options.algorithm,
                Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
            ),
            JwtKey::Pem { .. } => options.algorithm == Algorithm::EdDSA,
        };
        if !fits {
            let (key, fitting) = match &options.key {
                JwtKey::Hmac(_) => ("an HMAC secret", "HS256, HS384 or HS512"),
                JwtKey::Pem { .. } => ("an EdDSA key", "EdDSA"),
            };
            return Err(AuthError::Failed(format!(
                "the {:?} algorithm cannot be used with {key}: use {fitting}",
                options.algorithm
            )));
        }
        let (encoding, decoding) = match &options.key {
            JwtKey::Hmac(secret) => {
                // Fail closed at the derivation point every constructor reaches.
                if secret.trim().is_empty() {
                    return Err(AuthError::Failed(format!(
                        "{}, must not be empty",
                        crate::config::secret_setting()
                    )));
                }
                // RFC 7518 §3.2: a key of the same size as the hash output, or
                // larger — 256 bits for HS256, 384 for HS384, 512 for HS512.
                let minimum = min_hmac_secret_bytes(options.algorithm);
                if secret.len() < minimum {
                    return Err(AuthError::Failed(format!(
                        "{}, must be at least {minimum} bytes ({} bits) for {:?}; got {}",
                        crate::config::secret_setting(),
                        minimum * 8,
                        options.algorithm,
                        secret.len()
                    )));
                }
                let bytes = secret.as_bytes();
                (
                    Some(EncodingKey::from_secret(bytes)),
                    DecodingKey::from_secret(bytes),
                )
            }
            JwtKey::Pem {
                private_pem,
                public_pem,
            } => {
                let decoding = DecodingKey::from_ed_pem(public_pem.as_bytes()).map_err(|e| {
                    AuthError::Failed(format!(
                        "{}, is not an EdDSA public key in PEM form ({e})",
                        crate::config::spellings("PUBLIC_KEY", "public_key"),
                    ))
                })?;
                let encoding = match private_pem {
                    Some(pem) => {
                        let encoding = EncodingKey::from_ed_pem(pem.as_bytes()).map_err(|e| {
                            AuthError::Failed(format!(
                                "{}, is not an EdDSA private key in PEM form ({e})",
                                crate::config::spellings("PRIVATE_KEY", "private_key"),
                            ))
                        })?;
                        prove_one_pair(&encoding, &decoding, options.algorithm)?;
                        Some(encoding)
                    }
                    None => None,
                };
                (encoding, decoding)
            }
        };

        // A contradiction, refused rather than resolved.
        if options.allow_any_audience {
            if options.audience.is_some() {
                return Err(AuthError::Failed(format!(
                    "{} contradicts {}: an audience-agnostic verifier cannot also require an audience",
                    var_name(AuthnConfig::NAMESPACE, "ALLOW_ANY_AUDIENCE"),
                    var_name(AuthnConfig::NAMESPACE, "AUDIENCE"),
                )));
            }
            // Not silent: named once per boot, so the opt-out is actionable.
            tracing::warn!(
                target: crate::TARGET,
                var = %var_name(AuthnConfig::NAMESPACE, "ALLOW_ANY_AUDIENCE"),
                "audience validation is disabled — a token minted for another service verifies here",
            );
        }

        let mut validation = Validation::new(options.algorithm);
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
        if let Some(iss) = &options.issuer {
            validation.set_issuer(&[iss.as_str()]);
            validation.required_spec_claims.insert("iss".to_owned());
        }
        // No `iss` twin of `validate_aud`: RFC 7519 §4.1.1 states no such clause.

        // RFC 9068 §2.1 and §4 (explicit typing, RFC 8725 §3.11): mint `at+jwt`,
        // and `verify` refuses any other `typ`.
        let mut header = Header::new(options.algorithm);
        if options.explicit_typing {
            header.typ = Some(AT_JWT.to_owned());
        }
        Ok(Self {
            encoding,
            decoding,
            header,
            validation,
            explicit_typing: options.explicit_typing,
            expires_in: options.expires_in,
            audience: options.audience,
            issuer: options.issuer,
        })
    }

    /// Sign `claims` into a compact JWT. Errors on a verify-only service (no
    /// encoding key) or a serialization failure.
    pub fn sign<C: Serialize>(&self, claims: &C) -> Result<String, AuthError> {
        self.encode_with(&self.header, claims)
    }

    /// Sign `claims` under `header`, stamping `aud`/`iss` first.
    fn encode_with<C: Serialize>(&self, header: &Header, claims: &C) -> Result<String, AuthError> {
        let encoding = self.encoding.as_ref().ok_or_else(|| {
            AuthError::Failed("this JwtService is verify-only — no signing key configured".into())
        })?;
        match self.stamped(claims)? {
            Some(stamped) => encode(header, &stamped, encoding),
            None => encode(header, claims, encoding),
        }
        .map_err(|e| AuthError::Failed(e.to_string()))
    }

    /// Stamp the configured `aud` / `iss` onto a claims object.
    ///
    /// Verification requires both when configured, so a claims struct omitting
    /// them would mint tokens its own verifier rejects. `None` when neither is
    /// configured; a claims type's own `aud`/`iss` is never overwritten.
    fn stamped<C: Serialize>(&self, claims: &C) -> Result<Option<serde_json::Value>, AuthError> {
        if self.audience.is_none() && self.issuer.is_none() {
            return Ok(None);
        }
        let mut value =
            serde_json::to_value(claims).map_err(|e| AuthError::Failed(e.to_string()))?;
        let Some(map) = value.as_object_mut() else {
            // A non-object body cannot carry registered claims; the encoder fails it.
            return Ok(None);
        };
        for (key, configured) in [("aud", &self.audience), ("iss", &self.issuer)] {
            if let Some(v) = configured {
                map.entry(key)
                    .or_insert_with(|| serde_json::Value::String(v.clone()));
            }
        }
        Ok(Some(value))
    }

    /// Verify `token` and deserialize its claims into `C`, applying the pinned
    /// `exp`/`nbf`/`aud`/`iss` validation. Maps the failure to a typed
    /// [`AuthError`] (expired, bad signature, wrong algorithm, …).
    pub fn verify<C: DeserializeOwned>(&self, token: &str) -> Result<C, AuthError> {
        let data =
            decode::<C>(token, &self.decoding, &self.validation).map_err(map_decode_error)?;
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
    pub fn sign_handshake<C: Serialize>(
        &self,
        purpose: &str,
        claims: &C,
    ) -> Result<String, AuthError> {
        let mut header = self.header.clone();
        header.typ = Some(handshake_typ(purpose));
        self.encode_with(&header, claims)
    }

    /// Verify a handshake token minted by [`sign_handshake`](Self::sign_handshake)
    /// for the same `purpose`.
    ///
    /// The type check is unconditional — a flow's separation from access tokens
    /// cannot depend on a configuration flag — and it is exact, so a
    /// transaction minted for one flow does not verify on another's.
    pub fn verify_handshake<C: DeserializeOwned>(
        &self,
        purpose: &str,
        token: &str,
    ) -> Result<C, AuthError> {
        let data =
            decode::<C>(token, &self.decoding, &self.validation).map_err(map_decode_error)?;
        let found = data.header.typ.as_deref().unwrap_or_default();
        if !found.eq_ignore_ascii_case(&handshake_typ(purpose)) {
            return Err(AuthError::InvalidToken);
        }
        Ok(data.claims)
    }

    /// Absolute `exp` for a token minted now with the default TTL — the value
    /// callers put in a claims struct's `exp` field.
    pub fn expiry(&self) -> u64 {
        self.expiry_in(self.expires_in.as_secs())
    }

    /// Absolute `exp` for a token that should live exactly `secs` seconds —
    /// used for short-lived handshake tokens (e.g. the OAuth transaction) that
    /// must not inherit the full access-token TTL.
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
        tracing::debug!(target: crate::TARGET, error = %nest_rs_core::error_message(&err), "JWT verification failed");
    }
    mapped
}

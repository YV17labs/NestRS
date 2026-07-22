//! [`JwtService`] — sign and verify JSON Web Tokens.

use std::time::Duration;

use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode, errors::ErrorKind,
    get_current_timestamp,
};
use serde::{Serialize, de::DeserializeOwned};

use crate::error::AuthError;

/// Minimum HS256 shared-secret length: 256 bits (32 bytes). HMAC-SHA256 derives
/// all its security from the secret's entropy, so a shorter secret is
/// brute-forceable and mints forgeable tokens. Enforced in [`JwtService::new`]
/// — the derivation point every constructor funnels through (`JwtOptions::new`,
/// the honest-API path, included), not only on the config-env path.
pub(crate) const HS256_MIN_SECRET_BYTES: usize = 32;

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

/// Runtime JWT settings passed to [`AuthnModule::for_root`](super::AuthnModule::for_root).
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
    pub audience: Option<String>,
    /// When set, tokens must carry a matching `iss` claim.
    pub issuer: Option<String>,
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
    expires_in: Duration,
    /// Stamped onto every minted token when configured — `validation` requires
    /// them on the verifying side, so the signer must not leave them to the
    /// app's claims struct.
    audience: Option<String>,
    issuer: Option<String>,
}

impl JwtService {
    /// Build the service from [`JwtOptions`], deriving keys and pinning the
    /// validation policy (`exp`/`nbf` always checked; `aud`/`iss` required only
    /// when configured, and then also required-present so an omitting token
    /// fails closed). Errors on unparseable PEM key material.
    pub fn new(options: JwtOptions) -> Result<Self, AuthError> {
        let (encoding, decoding) = match &options.key {
            JwtKey::Hmac(secret) => {
                // Fail closed at the derivation point: an HS256 secret under 256
                // bits is brute-forceable. This guards every constructor path —
                // `JwtOptions::new` (the documented honest-API) as much as the
                // config-env path (SEC-F3).
                if secret.len() < HS256_MIN_SECRET_BYTES {
                    return Err(AuthError::Failed(format!(
                        "HS256 secret must be at least {HS256_MIN_SECRET_BYTES} bytes (256 bits); got {}",
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
                let decoding = DecodingKey::from_ed_pem(public_pem.as_bytes())
                    .map_err(|e| AuthError::Failed(format!("invalid JWT public key: {e}")))?;
                let encoding = match private_pem {
                    Some(pem) => {
                        Some(EncodingKey::from_ed_pem(pem.as_bytes()).map_err(|e| {
                            AuthError::Failed(format!("invalid JWT private key: {e}"))
                        })?)
                    }
                    None => None,
                };
                (encoding, decoding)
            }
        };

        let mut validation = Validation::new(options.algorithm);
        // Pin expiry validation explicitly — the most security-critical claim
        // check must not ride on a library default that a future version could
        // flip. (jsonwebtoken defaults this to `true` today; we state intent.)
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.leeway = options.leeway.as_secs();
        // `set_audience`/`set_issuer` only *compare* `aud`/`iss` when the token
        // carries them — a signed token omitting the claim would pass despite the
        // config promising it is mandatory. Add the claim to `required_spec_claims`
        // so an omitting token fails closed.
        match &options.audience {
            Some(aud) => {
                validation.set_audience(&[aud.as_str()]);
                validation.required_spec_claims.insert("aud".to_owned());
            }
            None => validation.validate_aud = false,
        }
        if let Some(iss) = &options.issuer {
            validation.set_issuer(&[iss.as_str()]);
            validation.required_spec_claims.insert("iss".to_owned());
        }

        Ok(Self {
            encoding,
            decoding,
            header: Header::new(options.algorithm),
            validation,
            expires_in: options.expires_in,
            audience: options.audience,
            issuer: options.issuer,
        })
    }

    /// Sign `claims` into a compact JWT. Errors on a verify-only service (no
    /// encoding key) or a serialization failure.
    pub fn sign<C: Serialize>(&self, claims: &C) -> Result<String, AuthError> {
        let encoding = self.encoding.as_ref().ok_or_else(|| {
            AuthError::Failed("this JwtService is verify-only — no signing key configured".into())
        })?;
        match self.stamped(claims)? {
            Some(stamped) => encode(&self.header, &stamped, encoding),
            None => encode(&self.header, claims, encoding),
        }
        .map_err(|e| AuthError::Failed(e.to_string()))
    }

    /// Stamp the configured `aud` / `iss` onto a claims object.
    ///
    /// Verification **requires** both when they are configured, so an app whose
    /// claims struct omits them would mint tokens its own verifier rejects —
    /// the kind of scoping the framework must carry rather than ask every
    /// claims type to restate. Returns `None` when there is nothing to stamp
    /// (neither configured), so the common path serializes exactly once. A
    /// claims type that sets its own `aud`/`iss` wins: an explicit value is
    /// never overwritten.
    fn stamped<C: Serialize>(&self, claims: &C) -> Result<Option<serde_json::Value>, AuthError> {
        if self.audience.is_none() && self.issuer.is_none() {
            return Ok(None);
        }
        let mut value =
            serde_json::to_value(claims).map_err(|e| AuthError::Failed(e.to_string()))?;
        let Some(map) = value.as_object_mut() else {
            // A non-object claims body cannot carry registered claims; leave it
            // to the encoder, which fails it the same way it always has.
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
        decode::<C>(token, &self.decoding, &self.validation)
            .map(|data| data.claims)
            .map_err(map_decode_error)
    }

    /// Absolute `exp` for a token minted now with the default TTL — the value
    /// callers put in a claims struct's `exp` field.
    pub fn expiry(&self) -> u64 {
        get_current_timestamp() + self.expires_in.as_secs()
    }

    /// Absolute `exp` for a token that should live exactly `secs` seconds —
    /// used for short-lived handshake tokens (e.g. the OAuth transaction) that
    /// must not inherit the full access-token TTL.
    pub fn expiry_in(&self, secs: u64) -> u64 {
        get_current_timestamp() + secs
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
    // The guard layer (`AuthnGuard::check_http`) emits the single `warn` for an
    // authentication failure with strategy + route context; this stays `debug`
    // so the typed decode reason is available without double-counting denials.
    if !matches!(mapped, AuthError::Expired) {
        tracing::debug!(target: "nest_rs::authn", error = %err, "JWT verification failed");
    }
    mapped
}

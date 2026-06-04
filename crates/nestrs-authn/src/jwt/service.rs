//! [`JwtService`] — sign and verify JSON Web Tokens.

use std::time::Duration;

use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode, errors::ErrorKind,
    get_current_timestamp,
};
use serde::{Serialize, de::DeserializeOwned};

use crate::error::AuthError;

/// Key material backing a [`JwtService`].
#[derive(Clone)]
pub enum JwtKey {
    /// Shared secret: the same key signs and verifies. Every verifier can also mint.
    Hmac(String),
    /// Asymmetric PEM keys. `private_pem` is `None` on a verify-only resource server.
    Pem {
        private_pem: Option<String>,
        public_pem: String,
    },
}

/// Runtime JWT settings passed to [`AuthnModule::for_root`](super::AuthnModule::for_root).
#[derive(Clone)]
pub struct JwtOptions {
    pub key: JwtKey,
    pub algorithm: Algorithm,
    pub expires_in: Duration,
    /// Clock skew tolerated when validating `exp` / `nbf`.
    pub leeway: Duration,
    /// When set, tokens must carry a matching `aud` claim.
    pub audience: Option<String>,
}

impl JwtOptions {
    const DEFAULT_LEEWAY: Duration = Duration::from_secs(30);

    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            key: JwtKey::Hmac(secret.into()),
            algorithm: Algorithm::HS256,
            expires_in: Duration::from_secs(3600),
            leeway: Self::DEFAULT_LEEWAY,
            audience: None,
        }
    }

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
        }
    }

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
        }
    }
}

pub struct JwtService {
    encoding: Option<EncodingKey>,
    decoding: DecodingKey,
    header: Header,
    validation: Validation,
    expires_in: Duration,
}

impl JwtService {
    pub fn new(options: JwtOptions) -> Result<Self, AuthError> {
        let (encoding, decoding) = match &options.key {
            JwtKey::Hmac(secret) => {
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
        validation.validate_nbf = true;
        validation.leeway = options.leeway.as_secs();
        match &options.audience {
            Some(aud) => validation.set_audience(&[aud.as_str()]),
            None => validation.validate_aud = false,
        }

        Ok(Self {
            encoding,
            decoding,
            header: Header::new(options.algorithm),
            validation,
            expires_in: options.expires_in,
        })
    }

    pub fn sign<C: Serialize>(&self, claims: &C) -> Result<String, AuthError> {
        let encoding = self.encoding.as_ref().ok_or_else(|| {
            AuthError::Failed("this JwtService is verify-only — no signing key configured".into())
        })?;
        encode(&self.header, claims, encoding).map_err(|e| AuthError::Failed(e.to_string()))
    }

    pub fn verify<C: DeserializeOwned>(&self, token: &str) -> Result<C, AuthError> {
        decode::<C>(token, &self.decoding, &self.validation)
            .map(|data| data.claims)
            .map_err(map_decode_error)
    }

    pub fn expiry(&self) -> u64 {
        get_current_timestamp() + self.expires_in.as_secs()
    }

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
    if !matches!(mapped, AuthError::Expired) {
        tracing::warn!(target: "nestrs::auth", error = %err, "JWT verification failed");
    }
    mapped
}

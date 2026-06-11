//! [`JwtConfig`] — env-driven JWT key material.

use std::time::Duration;

use nest_rs_config::{Config, ConfigService, config};
use validator::Validate;

use crate::error::AuthError;
use crate::jwt::JwtOptions;

/// Minimum HS256 secret length, in bytes. 256 bits matches the HMAC-SHA256
/// output size — anything shorter weakens the signature below the algorithm's
/// own security level.
const HS256_MIN_SECRET_BYTES: usize = 32;

// No `Debug`: secrets must not leak through a derived format.
#[config(namespace = "authn")]
#[derive(Clone, Default, Validate)]
pub struct JwtConfig {
    pub secret: Option<String>,
    pub private_key: Option<String>,
    pub public_key: Option<String>,
    /// Clock skew leeway in seconds (`NESTRS_AUTHN__LEEWAY_SECS`, default 30).
    pub leeway_secs: Option<u64>,
    /// Expected `aud` claim (`NESTRS_AUTHN__AUDIENCE`). Omitted ⇒ no audience check.
    pub audience: Option<String>,
    /// Expected `iss` claim (`NESTRS_AUTHN__ISSUER`). Omitted ⇒ no issuer check.
    pub issuer: Option<String>,
    /// Token lifetime in seconds (`NESTRS_AUTHN__EXPIRES_IN_SECS`, default 3600).
    pub expires_in_secs: Option<u64>,
}

impl Config for JwtConfig {
    fn from_env(env: &ConfigService) -> nest_rs_config::Result<Self> {
        Ok(Self {
            secret: env.get("SECRET"),
            private_key: env.get("PRIVATE_KEY"),
            public_key: env.get("PUBLIC_KEY"),
            leeway_secs: env.parse("LEEWAY_SECS")?,
            audience: env.get("AUDIENCE"),
            issuer: env.get("ISSUER"),
            expires_in_secs: env.parse("EXPIRES_IN_SECS")?,
        })
    }
}

impl JwtConfig {
    /// Infer signing mode from the keys present. Fails the boot when no usable combination exists.
    pub fn into_options(self) -> Result<JwtOptions, AuthError> {
        let leeway = Duration::from_secs(self.leeway_secs.unwrap_or(30));
        let audience = self.audience;
        let mut options = match (
            self.secret.as_ref(),
            self.private_key.as_ref(),
            self.public_key.as_ref(),
        ) {
            (Some(secret), Some(private), Some(public)) if !secret.trim().is_empty() => {
                tracing::warn!(
                    target: "nest_rs::auth",
                    secret_present = true,
                    eddsa_present = true,
                    "ignoring NESTRS_AUTHN__SECRET in favour of EdDSA keys"
                );
                JwtOptions::eddsa(private.clone(), public.clone())
            }
            (Some(secret), _, _) if secret.trim().is_empty() => {
                return Err(AuthError::Failed(
                    "NESTRS_AUTHN__SECRET must not be empty".into(),
                ));
            }
            // HS256 derives its security from the secret's entropy. A short
            // secret is brute-forceable, so refuse anything under 256 bits
            // (32 bytes) at boot rather than minting forgeable tokens.
            (Some(secret), _, _) if secret.len() < HS256_MIN_SECRET_BYTES => {
                return Err(AuthError::Failed(format!(
                    "NESTRS_AUTHN__SECRET must be at least {HS256_MIN_SECRET_BYTES} bytes for HS256"
                )));
            }
            (Some(secret), _, _) => JwtOptions::new(secret.clone()),
            (None, Some(private), Some(public)) => {
                JwtOptions::eddsa(private.clone(), public.clone())
            }
            (None, None, Some(public)) => JwtOptions::eddsa_verify(public.clone()),
            (None, Some(_), None) => {
                return Err(AuthError::Failed(
                    "NESTRS_AUTHN__PRIVATE_KEY is set without NESTRS_AUTHN__PUBLIC_KEY".into(),
                ));
            }
            (None, None, None) => {
                return Err(AuthError::Failed(
                    "no JWT key configured: set NESTRS_AUTHN__SECRET (HS256) or \
                     NESTRS_AUTHN__PUBLIC_KEY (EdDSA)"
                        .into(),
                ));
            }
        };
        options.leeway = leeway;
        options.audience = audience;
        options.issuer = self.issuer;
        if let Some(secs) = self.expires_in_secs {
            options.expires_in = Duration::from_secs(secs);
        }
        Ok(options)
    }
}

//! [`JwtConfig`] — env-driven JWT key material.

use std::time::Duration;

use nest_rs_config::{Config, ConfigService, config, var_name};

use crate::error::AuthError;
use crate::jwt::JwtOptions;
// Single source of truth: the min-secret rule is enforced in `JwtService::new`;
// the config path checks it too only to surface an env-var-named message.
use crate::jwt::service::HS256_MIN_SECRET_BYTES;

// No `Debug`: secrets must not leak through a derived format.
/// Env-driven JWT key material (namespace `authn`). The combination of keys
/// present selects the signing mode; see [`into_options`](Self::into_options).
/// No `Debug` derive: secrets must not leak through a format.
#[config(namespace = "authn")]
#[derive(Clone, Default)]
pub struct JwtConfig {
    /// HS256 shared secret (`NESTRS_AUTHN__SECRET`). Present ⇒ symmetric mode;
    /// must be ≥ 32 bytes. A verifier holding it can also mint tokens.
    pub secret: Option<String>,
    /// EdDSA signing key, PEM (`NESTRS_AUTHN__PRIVATE_KEY`). Set only on the app
    /// that issues tokens; requires `public_key` alongside it.
    pub private_key: Option<String>,
    /// EdDSA verification key, PEM (`NESTRS_AUTHN__PUBLIC_KEY`). A resource
    /// server holds only this — it can verify but not sign.
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
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            secret: env.get("SECRET").or(base.secret),
            private_key: env.get("PRIVATE_KEY").or(base.private_key),
            public_key: env.get("PUBLIC_KEY").or(base.public_key),
            leeway_secs: env.parse("LEEWAY_SECS")?.or(base.leeway_secs),
            audience: env.get("AUDIENCE").or(base.audience),
            issuer: env.get("ISSUER").or(base.issuer),
            expires_in_secs: env.parse("EXPIRES_IN_SECS")?.or(base.expires_in_secs),
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
                    target: crate::TARGET,
                    secret_present = true,
                    eddsa_present = true,
                    secret_var = %var_name("authn", "SECRET"),
                    "ignoring the shared secret in favour of EdDSA keys"
                );
                JwtOptions::eddsa(private.clone(), public.clone())
            }
            (Some(secret), _, _) if secret.trim().is_empty() => {
                return Err(AuthError::Failed(format!(
                    "{} must not be empty",
                    var_name("authn", "SECRET"),
                )));
            }
            // HS256 derives its security from the secret's entropy. A short
            // secret is brute-forceable, so refuse anything under 256 bits
            // (32 bytes) at boot rather than minting forgeable tokens.
            (Some(secret), _, _) if secret.len() < HS256_MIN_SECRET_BYTES => {
                return Err(AuthError::Failed(format!(
                    "{} must be at least {HS256_MIN_SECRET_BYTES} bytes for HS256",
                    var_name("authn", "SECRET"),
                )));
            }
            (Some(secret), _, _) => JwtOptions::new(secret.clone()),
            (None, Some(private), Some(public)) => {
                JwtOptions::eddsa(private.clone(), public.clone())
            }
            (None, None, Some(public)) => JwtOptions::eddsa_verify(public.clone()),
            (None, Some(_), None) => {
                return Err(AuthError::Failed(format!(
                    "{} is set without {}",
                    var_name("authn", "PRIVATE_KEY"),
                    var_name("authn", "PUBLIC_KEY"),
                )));
            }
            (None, None, None) => {
                return Err(AuthError::Failed(format!(
                    "no JWT key configured: set {} (HS256) or {} (EdDSA)",
                    var_name("authn", "SECRET"),
                    var_name("authn", "PUBLIC_KEY"),
                )));
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

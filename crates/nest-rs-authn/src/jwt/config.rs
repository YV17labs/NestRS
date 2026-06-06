//! [`JwtConfig`] — env-driven JWT key material.

use std::time::Duration;

use nest_rs_config::{Config, ConfigService, config};
use validator::Validate;

use crate::error::AuthError;
use crate::jwt::JwtOptions;

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
}

impl Config for JwtConfig {
    fn from_env(env: &ConfigService) -> nest_rs_config::Result<Self> {
        Ok(Self {
            secret: env.get("SECRET"),
            private_key: env.get("PRIVATE_KEY"),
            public_key: env.get("PUBLIC_KEY"),
            leeway_secs: env.parse("LEEWAY_SECS")?,
            audience: env.get("AUDIENCE"),
        })
    }
}

impl JwtConfig {
    /// Infer signing mode from the keys present. Fails the boot when no usable combination exists.
    pub fn into_options(self) -> Result<JwtOptions, AuthError> {
        let leeway = Duration::from_secs(self.leeway_secs.unwrap_or(30));
        let audience = self.audience;
        let mut options = match (self.secret, self.private_key, self.public_key) {
            (Some(secret), _, _) => JwtOptions::new(secret),
            (None, Some(private), Some(public)) => JwtOptions::eddsa(private, public),
            (None, None, Some(public)) => JwtOptions::eddsa_verify(public),
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
        Ok(options)
    }
}

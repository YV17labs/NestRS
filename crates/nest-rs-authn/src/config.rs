//! [`JwtConfig`] — env-driven JWT key material.

use std::time::Duration;

use nest_rs_config::{Config, ConfigService, Namespaced, config, var_name};

use crate::JwtOptions;
use crate::error::AuthError;
// Single source of truth: the min-secret rule is enforced in `JwtService::new`;
// the config path checks it too only to surface an env-var-named message.
use crate::service::HS256_MIN_SECRET_BYTES;

// No `Debug`: secrets must not leak through a derived format.
/// Env-driven JWT key material (namespace `authn`). The combination of keys
/// present selects the signing mode; see [`into_options`](Self::into_options).
/// No `Debug` derive: secrets must not leak through a format.
#[config(namespace = "authn")]
#[derive(Clone, Default)]
pub struct JwtConfig {
    /// HS256 shared secret (key `SECRET`). Present ⇒ symmetric mode;
    /// must be ≥ 32 bytes. A verifier holding it can also mint tokens.
    pub secret: Option<String>,
    /// EdDSA signing key, PEM (key `PRIVATE_KEY`). Set only on the app
    /// that issues tokens; requires `public_key` alongside it.
    pub private_key: Option<String>,
    /// EdDSA verification key, PEM (key `PUBLIC_KEY`). A resource
    /// server holds only this — it can verify but not sign.
    pub public_key: Option<String>,
    /// Clock skew leeway in seconds (key `LEEWAY_SECS`, default 30).
    pub leeway_secs: Option<u64>,
    /// Expected `aud` claim (key `AUDIENCE`). Set ⇒ the claim is **mandatory**
    /// and must name this service.
    ///
    /// **Omitting it is not omitting the check.** RFC 7519 §4.1.3 obliges a
    /// verifier to reject a token that *carries* an `aud` it is not named in,
    /// and that clause binds a service naming no audience of its own too — so
    /// an unconfigured verifier accepts a token with no `aud` and refuses one
    /// minted for a sibling service by the same issuer. What configuring it
    /// adds is the *other* direction: a token omitting `aud` entirely then
    /// fails closed as well. The opt-out is
    /// [`allow_any_audience`](Self::allow_any_audience), and only that.
    pub audience: Option<String>,
    /// Expected `iss` claim (key `ISSUER`). Omitted ⇒ no issuer check.
    pub issuer: Option<String>,
    /// Token lifetime in seconds (key `EXPIRES_IN_SECS`, default 3600).
    pub expires_in_secs: Option<u64>,
    /// Opt out of RFC 7519 §4.1.3 (key `ALLOW_ANY_AUDIENCE`, default `false`):
    /// accept a token whose `aud` names a service this one is not.
    ///
    /// Named, explicit and off by default, because the behaviour it restores is
    /// the confused deputy — any app the issuer mints for becomes a credential
    /// for this one. It is refused beside [`audience`](Self::audience), which
    /// declares the opposite, and it reports itself at `warn` once per boot.
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
                    secret_var = %var_name(Self::NAMESPACE, "SECRET"),
                    "ignoring the shared secret in favour of EdDSA keys"
                );
                JwtOptions::eddsa(private.clone(), public.clone())
            }
            (Some(secret), _, _) if secret.trim().is_empty() => {
                return Err(AuthError::Failed(format!(
                    "{} must not be empty",
                    var_name(Self::NAMESPACE, "SECRET"),
                )));
            }
            // HS256 derives its security from the secret's entropy. A short
            // secret is brute-forceable, so refuse anything under 256 bits
            // (32 bytes) at boot rather than minting forgeable tokens.
            (Some(secret), _, _) if secret.len() < HS256_MIN_SECRET_BYTES => {
                return Err(AuthError::Failed(format!(
                    "{} must be at least {HS256_MIN_SECRET_BYTES} bytes for HS256",
                    var_name(Self::NAMESPACE, "SECRET"),
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
                    var_name(Self::NAMESPACE, "PRIVATE_KEY"),
                    var_name(Self::NAMESPACE, "PUBLIC_KEY"),
                )));
            }
            (None, None, None) => {
                return Err(AuthError::Failed(format!(
                    "no JWT key configured: set {} (HS256) or {} (EdDSA)",
                    var_name(Self::NAMESPACE, "SECRET"),
                    var_name(Self::NAMESPACE, "PUBLIC_KEY"),
                )));
            }
        };
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

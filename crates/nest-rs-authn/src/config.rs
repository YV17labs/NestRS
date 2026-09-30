//! [`JwtConfig`] — env-driven JWT key material.

use std::time::Duration;

use nest_rs_config::{
    Bound, Config, ConfigService, DurationBounds, DurationUnit, Floor, Namespaced, config,
};

use crate::JwtOptions;
use crate::error::AuthError;
// Single source of truth: the min-secret rule is enforced in `JwtService::new`;
// the config path checks it too only to surface an env-var-named message.

/// The token lifetime's range, the variable that sets it, and why.
pub(crate) const EXPIRES_IN: DurationBounds = DurationBounds {
    key: "EXPIRES_IN_SECS",
    field: "JwtConfig::expires_in_secs",
    unit: DurationUnit::Seconds,
    least: Floor::Units(Bound {
        count: 1,
        why: "a token that expires as it is minted is refused by every verifier, so every \
              sign-in would succeed and hand out nothing usable",
    }),
    most: Some(Bound {
        count: 30 * 24 * 60 * 60,
        why: "an access token is a bearer credential until it expires and nothing revokes it \
              sooner, so a lifetime past thirty days is a unit slip or a credential that never \
              ends — a long session is a refresh flow, not a long token",
    }),
};

/// The clock-skew leeway's range, the variable that sets it, and why.
pub(crate) const LEEWAY: DurationBounds = DurationBounds {
    key: "LEEWAY_SECS",
    field: "JwtConfig::leeway_secs",
    unit: DurationUnit::Seconds,
    least: Floor::Units(Bound {
        count: 0,
        why: "no leeway is a leeway",
    }),
    most: Some(Bound {
        count: 5 * 60,
        why: "RFC 7519 §4.1.4 allows \"no more than a few minutes\" for clock skew, and every \
              second past it is a second an expired token still verifies",
    }),
};

// No `Debug`: secrets must not leak through a derived format.
/// Env-driven JWT key material (namespace `authn`). The combination of keys
/// present selects the signing mode; see [`into_options`](Self::into_options).
/// No `Debug` derive: secrets must not leak through a format.
#[config(namespace = "authn")]
#[derive(Clone, Default)]
pub struct JwtConfig {
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
    /// Clock skew leeway in seconds (key `LEEWAY_SECS`, default 30), at most
    /// 300 — RFC 7519 §4.1.4's "a few minutes".
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
    /// Token lifetime in seconds (key `EXPIRES_IN_SECS`, default 3600), at
    /// least 1 and at most thirty days.
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
            secret: env.get("SECRET")?.or(base.secret),
            private_key: pem_text(env, "PRIVATE_KEY")?.or(base.private_key),
            public_key: pem_text(env, "PUBLIC_KEY")?.or(base.public_key),
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

/// A PEM key's setting, every way it can be given. By the time a [`JwtConfig`]
/// is judged, which spelling supplied a field — or whether code pinned it — is
/// no longer known, so the message names the setting rather than claim one
/// variable that may not exist. A sentence continuing past it sets it off with
/// commas.
pub(crate) fn spellings(key: &str, field: &str) -> String {
    format!(
        "{}, or `{field}` in a JwtConfig or JwtOptions built in code",
        nest_rs_config::spellings(JwtConfig::NAMESPACE, key),
    )
}

/// The secret's setting, every way it can be given — the same shape as
/// [`spellings`], since a secret is a variable like any other and takes its
/// `_FILE` spelling too.
pub(crate) fn secret_setting() -> String {
    spellings("SECRET", "secret")
}

impl JwtConfig {
    /// Infer signing mode from the keys present. Fails the boot when no usable
    /// combination exists.
    ///
    /// Each field was read on its own, from whichever tier set it, so the
    /// combination is judged here, whole, and never chosen from: a pair signs
    /// and verifies EdDSA, a public key alone verifies it, a secret alone signs
    /// and verifies HS256.
    ///
    /// The refusals run in one order, so the first thing an operator reads is
    /// the most fundamental: a secret — any value, empty included — beside
    /// either key (two signing modes, nothing says which is meant), naming every
    /// setting that is set; then a private key without its public key; then
    /// nothing set at all. Whether each value is usable is judged once, by
    /// [`JwtService::new`](crate::JwtService::new).
    pub fn into_options(self) -> Result<JwtOptions, AuthError> {
        let leeway = Duration::from_secs(self.leeway_secs.unwrap_or(30));
        let audience = self.audience;
        let (secret, private, public) = (self.secret, self.private_key, self.public_key);
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
        // Only the combination is judged here — which key the settings make.
        // Whether each value is usable (a secret long enough, PEM that parses, two
        // keys of one pair) is `JwtService::new`'s, the one constructor a config
        // and a value built in code both reach.
        let mut options = match (secret, private, public) {
            (Some(secret), _, _) => JwtOptions::new(secret),
            (None, private, Some(public)) => match private {
                Some(private) => JwtOptions::eddsa(private, public),
                None => JwtOptions::eddsa_verify(public),
            },
            // A private key alone was refused above, so only nothing is left.
            (None, _, None) => {
                return Err(AuthError::Failed(format!(
                    "no JWT key configured: set {} for HS256, or {} for EdDSA — or the same \
                     field in a JwtConfig or JwtOptions built in code",
                    nest_rs_config::spellings(Self::NAMESPACE, "SECRET"),
                    nest_rs_config::spellings(Self::NAMESPACE, "PUBLIC_KEY"),
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

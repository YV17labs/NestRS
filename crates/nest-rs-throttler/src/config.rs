//! [`ThrottlerConfig`] — rate-limit settings populated from `<PREFIX>_THROTTLER__*`.

use std::time::Duration;

use nest_rs_config::{
    Bound, Config, ConfigError, ConfigService, DurationBounds, Floor, Result, config, var_name,
};

use crate::pseudonym::MIN_KEY_BYTES;

/// The window's range, the variable that sets it, and why.
const WINDOW: DurationBounds = DurationBounds::secs(
    "WINDOW_SECS",
    "ThrottlerConfig::window_secs",
    Floor::Units(Bound {
        count: 1,
        why: "a zero window resets every bucket on every hit, so the count never passes one and \
              every request is allowed at any limit",
    }),
    Bound {
        count: 24 * 60 * 60,
        why: "a window past a day is a quota rather than a rate, and a quota kept in a store \
              that a restart or an eviction forgets is not one — count it in the database",
    },
);

/// The key the pseudonym key is read from, beside its `_FILE` spelling.
const PSEUDONYM_KEY: &str = "PSEUDONYM_KEY";

/// Rate-limit settings, settable via `<PREFIX>_THROTTLER__*` or pinned through
/// [`ThrottlerModule::for_root`](crate::ThrottlerModule::for_root).
///
/// No `Debug` derive: the pseudonym key is a secret, and its `Debug` says only
/// whether it is set.
#[config(namespace = "throttler")]
#[derive(Clone, Default)]
pub struct ThrottlerConfig {
    /// Requests allowed per window. Unset ⇒ module default (60).
    pub limit: Option<u32>,
    /// Window size in whole seconds, from 1 to 86400 (a day). Unset ⇒ module
    /// default (60).
    pub window_secs: Option<u64>,
    /// The key a store outside this process counts each bucket under: an
    /// HMAC-SHA256 of the bucket's subject, so the client's address or identity
    /// never reaches it. At least 32 bytes, the same on every replica sharing
    /// the store; required beside one (`RedisThrottlerModule`), unused by the
    /// in-process default. `<PREFIX>_THROTTLER__PSEUDONYM_KEY`, or its `_FILE`.
    pub pseudonym_key: Option<String>,
}

impl std::fmt::Debug for ThrottlerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThrottlerConfig")
            .field("limit", &self.limit)
            .field("window_secs", &self.window_secs)
            .field(
                "pseudonym_key",
                &self.pseudonym_key.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Why a pseudonym key shorter than [`MIN_KEY_BYTES`] is refused, without its
/// value.
const SHORT_KEY: &str = "must be at least 32 bytes: HMAC-SHA256's key is as strong as its \
                         length up to the hash's size (RFC 2104 §3) — `openssl rand -base64 32` \
                         gives one";

// Trusted proxies live on `HttpConfig` (`<PREFIX>_HTTP__TRUSTED_PROXIES`): they
// decide who every request is attributed to, not only the bucket.

impl Config for ThrottlerConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let pseudonym_key = match env.setting(PSEUDONYM_KEY)? {
            Some(setting) if setting.value.len() < MIN_KEY_BYTES => {
                return Err(setting.refuse(SHORT_KEY));
            }
            Some(setting) => Some(setting.value),
            None => match base.pseudonym_key {
                Some(pinned) if pinned.len() < MIN_KEY_BYTES => {
                    return Err(ConfigError::parse(
                        var_name("throttler", PSEUDONYM_KEY),
                        format!("`ThrottlerConfig::pseudonym_key` set in code {SHORT_KEY}"),
                    ));
                }
                pinned => pinned,
            },
        };
        Ok(Self {
            limit: env.parse("LIMIT")?.or(base.limit),
            window_secs: WINDOW
                .read_optional(env, base.window_secs.map(Duration::from_secs))?
                .map(|read| read.value.as_secs()),
            pseudonym_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_no_env_set() {
        let cfg = ThrottlerConfig::from_env(
            &ConfigService::with_vars("throttler", []),
            Default::default(),
        )
        .expect("no error");
        assert!(cfg.limit.is_none(), "unset ⇒ module default applies later");
        assert!(cfg.window_secs.is_none());
    }

    #[test]
    fn env_overrides_each_field_of_a_pinned_config() {
        let pinned = ThrottlerConfig {
            limit: Some(10),
            window_secs: Some(5),
            pseudonym_key: None,
        };
        let cfg = ThrottlerConfig::from_env(
            &ConfigService::with_vars("throttler", [("LIMIT", "120")]),
            pinned,
        )
        .expect("the overlay resolves");
        assert_eq!(cfg.limit, Some(120), "the env outranks the pin");
        assert_eq!(cfg.window_secs, Some(5), "the untouched pin survives");
    }

    /// A zero window would turn the limiter off on both stores.
    #[test]
    fn a_zero_window_is_refused_from_either_side() {
        let var = nest_rs_config::var_name("throttler", "WINDOW_SECS");
        let from_env = ThrottlerConfig::from_env(
            &ConfigService::with_vars("throttler", [("LIMIT", "1"), ("WINDOW_SECS", "0")]),
            Default::default(),
        )
        .expect_err("a zero window fails the boot")
        .to_string();
        assert!(
            from_env.contains(&var) && from_env.contains("must be at least 1 second"),
            "{from_env}"
        );
        let pinned = ThrottlerConfig::from_env(
            &ConfigService::with_vars("throttler", []),
            ThrottlerConfig {
                limit: Some(1),
                window_secs: Some(0),
                pseudonym_key: None,
            },
        )
        .expect_err("a pinned zero window fails the boot")
        .to_string();
        assert!(
            pinned.contains(&var)
                && pinned.contains("`ThrottlerConfig::window_secs` set in code is 0ns"),
            "{pinned}"
        );
    }

    #[test]
    fn a_window_past_a_day_is_refused_from_either_side() {
        let var = nest_rs_config::var_name("throttler", "WINDOW_SECS");
        let from_env = ThrottlerConfig::from_env(
            &ConfigService::with_vars("throttler", [("WINDOW_SECS", "86401")]),
            Default::default(),
        )
        .expect_err("past a day")
        .to_string();
        assert!(
            from_env.contains(&var) && from_env.contains("must be at most 86400 seconds"),
            "{from_env}"
        );
        let pinned = ThrottlerConfig::from_env(
            &ConfigService::with_vars("throttler", []),
            ThrottlerConfig {
                limit: None,
                window_secs: Some(u64::MAX),
                pseudonym_key: None,
            },
        )
        .expect_err("a pinned window past a day")
        .to_string();
        assert!(
            pinned.contains("above the 86400s it must be at most"),
            "{pinned}"
        );
    }

    /// A key short enough to guess would let anyone holding the store recover
    /// each client's address by hashing every address there is.
    #[test]
    fn a_short_pseudonym_key_is_refused_from_either_side_without_its_value() {
        let var = nest_rs_config::var_name("throttler", "PSEUDONYM_KEY");
        let short = "thirty-one bytes, one too few!!";
        assert_eq!(short.len(), MIN_KEY_BYTES - 1);
        let from_env = ThrottlerConfig::from_env(
            &ConfigService::with_vars("throttler", [("PSEUDONYM_KEY", short)]),
            Default::default(),
        )
        .expect_err("a 31-byte key fails the boot")
        .to_string();
        assert!(
            from_env.contains(&var) && from_env.contains("at least 32 bytes"),
            "{from_env}"
        );
        assert!(!from_env.contains(short), "{from_env}");
        let pinned = ThrottlerConfig::from_env(
            &ConfigService::with_vars("throttler", []),
            ThrottlerConfig {
                pseudonym_key: Some(short.to_owned()),
                ..ThrottlerConfig::default()
            },
        )
        .expect_err("a pinned 31-byte key fails the boot")
        .to_string();
        assert!(
            pinned.contains(&var)
                && pinned.contains("`ThrottlerConfig::pseudonym_key` set in code"),
            "{pinned}"
        );
        assert!(!pinned.contains(short), "{pinned}");
    }

    #[test]
    fn a_pseudonym_key_is_read_and_never_shown() {
        let key = "thirty-two bytes, exactly enough";
        let cfg = ThrottlerConfig::from_env(
            &ConfigService::with_vars("throttler", [("PSEUDONYM_KEY", key)]),
            Default::default(),
        )
        .expect("a 32-byte key is read");
        assert_eq!(cfg.pseudonym_key.as_deref(), Some(key));
        let shown = format!("{cfg:?}");
        assert!(
            !shown.contains(key) && shown.contains("<redacted>"),
            "{shown}"
        );
    }

    #[test]
    fn from_env_reads_all_fields_when_set() {
        let cfg = ThrottlerConfig::from_env(
            &ConfigService::with_vars("throttler", [("LIMIT", "120"), ("WINDOW_SECS", "90")]),
            Default::default(),
        )
        .expect("no error");
        assert_eq!(cfg.limit, Some(120));
        assert_eq!(cfg.window_secs, Some(90));
    }
}

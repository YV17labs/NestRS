//! [`ThrottlerConfig`] — rate-limit settings populated from `<PREFIX>_THROTTLER__*`.

use std::time::Duration;

use nest_rs_config::{
    Bound, ConfigError, ConfigService, CountBounds, DurationBounds, Field, Floor, Result, config,
};
use nest_rs_core::Secret;

use crate::pseudonym::MIN_KEY_BYTES;
use crate::throttle::DEFAULT_THROTTLE;

/// The window's range, and why.
const WINDOW: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "a zero window resets every bucket on every hit, so the count never passes one and \
         every request is allowed at any limit",
    )),
    Bound::new(
        24 * 60 * 60,
        "a window past a day is a quota rather than a rate, and a quota kept in a store \
         that a restart or an eviction forgets is not one — count it in the database",
    ),
);

const LIMIT: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a limit of none denies every request, which a guard says rather than a rate",
    ),
    Bound::new(
        1_000_000_000,
        "a client past a billion requests in a window, over eleven thousand a second for the \
         longest window there is, is no client a limit holds, so a number that large is a slip \
         that turns the limiter off in all but name",
    ),
);

/// Rate-limit settings, read by [`ThrottlerModule`](crate::ThrottlerModule)
/// from `<PREFIX>_THROTTLER__*` over a base
/// [`ThrottlerModule::for_root`](crate::ThrottlerModule::for_root) may pin.
#[config(namespace = "throttler")]
#[derive(Clone, Debug)]
pub struct ThrottlerConfig {
    /// Requests allowed per window, `<PREFIX>_THROTTLER__LIMIT`, from 1 to a
    /// billion — refused outside, from the environment and from the pinned
    /// struct alike; defaults to 60, [`DEFAULT_THROTTLE`]'s.
    #[config(bounds = LIMIT)]
    pub limit: u32,
    /// The window a bucket counts over, `<PREFIX>_THROTTLER__WINDOW_SECS` in
    /// whole seconds, from 1 to 86400 (a day); defaults to a minute,
    /// [`DEFAULT_THROTTLE`]'s.
    #[config(bounds = WINDOW)]
    pub window: Duration,
    /// The key a store outside this process counts each bucket under: an
    /// HMAC-SHA256 of the bucket's subject, so the client's address or identity
    /// never reaches it. At least 32 bytes, the same on every replica sharing
    /// the store; required beside one (`RedisThrottlerModule`), unused by the
    /// in-process default. `<PREFIX>_THROTTLER__PSEUDONYM_KEY`, or its `_FILE`.
    #[config(with = pseudonym_key)]
    pub pseudonym_key: Option<Secret<String>>,
}

impl Default for ThrottlerConfig {
    fn default() -> Self {
        Self {
            limit: DEFAULT_THROTTLE.limit(),
            window: DEFAULT_THROTTLE.window(),
            pseudonym_key: None,
        }
    }
}

/// Why a pseudonym key shorter than [`MIN_KEY_BYTES`] is refused, without its
/// value.
const SHORT_KEY: &str = "must be at least 32 bytes: HMAC-SHA256's key is as strong as its \
                         length up to the hash's size (RFC 2104 §3) — `openssl rand -base64 32` \
                         gives one";

// Trusted proxies live on `HttpConfig` (`<PREFIX>_HTTP__TRUSTED_PROXIES`): they
// decide who every request is attributed to, not only the bucket.

/// The pseudonym key over `base`, refused shorter than [`MIN_KEY_BYTES`] from
/// either side, never shown.
fn pseudonym_key(
    env: &ConfigService,
    field: Field,
    base: Option<Secret<String>>,
) -> Result<Option<Secret<String>>> {
    match env.setting(field.key())? {
        Some(setting) if setting.value.len() < MIN_KEY_BYTES => Err(setting.refuse(SHORT_KEY)),
        Some(setting) => Ok(Some(Secret::new(setting.value))),
        None => match base {
            Some(pinned) if pinned.expose_secret().len() < MIN_KEY_BYTES => {
                Err(ConfigError::parse(
                    env.var_name(field.key()),
                    format!("`{}` set in code {SHORT_KEY}", field.path()),
                ))
            }
            pinned => Ok(pinned),
        },
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_config::Config;

    use super::*;

    #[test]
    fn defaults_when_no_env_set() {
        let cfg = ThrottlerConfig::from_env(
            &ConfigService::with_vars("throttler", []),
            Default::default(),
        )
        .expect("no error");
        assert_eq!(cfg.limit, 60, "unset ⇒ the port's default");
        assert_eq!(cfg.window, Duration::from_secs(60));
    }

    #[test]
    fn env_overrides_each_field_of_a_pinned_config() {
        let pinned = ThrottlerConfig {
            limit: 10,
            window: Duration::from_secs(5),
            pseudonym_key: None,
        };
        let cfg = ThrottlerConfig::from_env(
            &ConfigService::with_vars("throttler", [("LIMIT", "120")]),
            pinned,
        )
        .expect("the overlay resolves");
        assert_eq!(cfg.limit, 120, "the env outranks the pin");
        assert_eq!(
            cfg.window,
            Duration::from_secs(5),
            "the untouched pin survives"
        );
    }

    /// One request to a billion in a window, from the environment and from a
    /// pin, naming the variable and its range and never the value it was given.
    #[test]
    fn the_limit_is_held_to_its_range_from_either_side() {
        let var = nest_rs_config::var_name("throttler", "LIMIT");
        for (raw, said) in [
            ("0", "must be at least 1 — "),
            ("1000000001", "must be at most 1000000000 — "),
        ] {
            let refused = ThrottlerConfig::from_env(
                &ConfigService::with_vars("throttler", [("LIMIT", raw)]),
                ThrottlerConfig::default(),
            )
            .expect_err("outside the range")
            .to_string();
            assert!(
                refused.contains(&var) && refused.contains(said),
                "{refused}"
            );
            assert!(!refused.contains("1000000001"), "{refused}");
        }
        for (pinned, said) in [
            (0, "below the 1 it must be at least"),
            (1_000_000_001, "above the 1000000000 it must be at most"),
        ] {
            let refused = ThrottlerConfig::from_env(
                &ConfigService::with_vars("throttler", []),
                ThrottlerConfig {
                    limit: pinned,
                    ..ThrottlerConfig::default()
                },
            )
            .expect_err("a pin outside the range")
            .to_string();
            assert!(
                refused.contains(&var)
                    && refused.contains("`ThrottlerConfig::limit` set in code")
                    && refused.contains(said),
                "{refused}"
            );
        }
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
                limit: 1,
                window: Duration::ZERO,
                pseudonym_key: None,
            },
        )
        .expect_err("a pinned zero window fails the boot")
        .to_string();
        assert!(
            pinned.contains(&var)
                && pinned.contains("`ThrottlerConfig::window` set in code is 0ns"),
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
                window: Duration::MAX,
                ..ThrottlerConfig::default()
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
                pseudonym_key: Some(short.into()),
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
        assert_eq!(
            cfg.pseudonym_key
                .as_ref()
                .map(Secret::expose_secret)
                .map(String::as_str),
            Some(key)
        );
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
        assert_eq!(cfg.limit, 120);
        assert_eq!(cfg.window, Duration::from_secs(90));
    }
}

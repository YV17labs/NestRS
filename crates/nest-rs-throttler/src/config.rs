//! [`ThrottlerConfig`] — rate-limit settings populated from `<PREFIX>_THROTTLER__*`.

use std::time::Duration;

use nest_rs_config::{Bound, Config, ConfigService, DurationBounds, Floor, Result, config};

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

/// Rate-limit settings, settable via `<PREFIX>_THROTTLER__*` or pinned through
/// [`ThrottlerModule::for_root`](crate::ThrottlerModule::for_root).
#[config(namespace = "throttler")]
#[derive(Clone, Debug, Default)]
pub struct ThrottlerConfig {
    /// Requests allowed per window. Unset ⇒ module default (60).
    pub limit: Option<u32>,
    /// Window size in whole seconds, from 1 to 86400 (a day). Unset ⇒ module
    /// default (60).
    pub window_secs: Option<u64>,
}

// Trusted proxies live on `HttpConfig` (`<PREFIX>_HTTP__TRUSTED_PROXIES`), not
// here: which reverse proxies a deployment believes decides who *every* request
// is attributed to, the `ClientIp` extractor's answer as much as the bucket's.

impl Config for ThrottlerConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            limit: env.parse("LIMIT")?.or(base.limit),
            window_secs: WINDOW
                .read_optional(env, base.window_secs.map(Duration::from_secs))?
                .map(|read| read.value.as_secs()),
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
        };
        let cfg = ThrottlerConfig::from_env(
            &ConfigService::with_vars("throttler", [("LIMIT", "120")]),
            pinned,
        )
        .expect("the overlay resolves");
        assert_eq!(cfg.limit, Some(120), "the env outranks the pin");
        assert_eq!(cfg.window_secs, Some(5), "the untouched pin survives");
    }

    /// A zero window turned the limiter off on both stores with no word; it is
    /// refused naming the variable, from the environment or pinned in code.
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

    /// Every duration a deployment sets has a ceiling: a window past a day is
    /// refused naming the variable, from either side.
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
            },
        )
        .expect_err("a pinned window past a day")
        .to_string();
        assert!(
            pinned.contains("above the 86400s it must be at most"),
            "{pinned}"
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

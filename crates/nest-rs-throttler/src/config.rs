//! [`ThrottlerConfig`] — rate-limit settings populated from `NESTRS_THROTTLER__*`.

use nest_rs_config::{Config, ConfigService, Result, config};

/// Rate-limit settings, settable via `NESTRS_THROTTLER__*` or pinned through
/// [`ThrottlerModule::for_root`](crate::ThrottlerModule::for_root).
#[config(namespace = "throttler")]
#[derive(Clone, Debug, Default)]
pub struct ThrottlerConfig {
    /// Requests allowed per window. Unset ⇒ module default (60).
    pub limit: Option<u32>,
    /// Window size in whole seconds. Unset ⇒ module default (60).
    pub window_secs: Option<u64>,
}

// Trusted proxies live on `HttpConfig` (`NESTRS_HTTP__TRUSTED_PROXIES`), not
// here: which reverse proxies a deployment believes decides who *every* request
// is attributed to, the `ClientIp` extractor's answer as much as the bucket's.

impl Config for ThrottlerConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            limit: env.parse("LIMIT")?.or(base.limit),
            window_secs: env.parse("WINDOW_SECS")?.or(base.window_secs),
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

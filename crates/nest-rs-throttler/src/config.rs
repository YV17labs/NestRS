//! [`ThrottlerConfig`] — rate-limit settings populated from `NESTRS_THROTTLER__*`.

use nest_rs_config::{Config, ConfigService, Result, config};
use validator::Validate;

/// Rate-limit settings, settable via `NESTRS_THROTTLER__*` or pinned through
/// [`ThrottlerModule::for_root`](crate::ThrottlerModule::for_root).
#[config(namespace = "throttler")]
#[derive(Clone, Debug, Default, Validate)]
pub struct ThrottlerConfig {
    /// Requests allowed per window. Unset ⇒ module default (60).
    pub limit: Option<u32>,
    /// Window size in whole seconds. Unset ⇒ module default (60).
    pub window_secs: Option<u64>,
    /// Proxies whose `X-Forwarded-For` chain may supply the client IP. An
    /// unparseable IP aborts the boot.
    pub trusted_proxies: Vec<String>,
}

impl Config for ThrottlerConfig {
    fn from_env(env: &ConfigService) -> Result<Self> {
        Ok(Self {
            limit: env.parse("LIMIT")?,
            window_secs: env.parse("WINDOW_SECS")?,
            trusted_proxies: env.list("TRUSTED_PROXIES"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_no_env_set() {
        let cfg = ThrottlerConfig::from_env(&ConfigService::with_vars("throttler", []))
            .expect("no error");
        assert!(cfg.limit.is_none(), "unset ⇒ module default applies later");
        assert!(cfg.window_secs.is_none());
        assert!(cfg.trusted_proxies.is_empty());
    }

    #[test]
    fn from_env_reads_all_fields_when_set() {
        let cfg = ThrottlerConfig::from_env(&ConfigService::with_vars(
            "throttler",
            [
                ("NESTRS_THROTTLER__LIMIT", "120"),
                ("NESTRS_THROTTLER__WINDOW_SECS", "90"),
                ("NESTRS_THROTTLER__TRUSTED_PROXIES", "10.0.0.1,192.168.0.1"),
            ],
        ))
        .expect("no error");
        assert_eq!(cfg.limit, Some(120));
        assert_eq!(cfg.window_secs, Some(90));
        assert_eq!(
            cfg.trusted_proxies,
            vec!["10.0.0.1".to_string(), "192.168.0.1".into()],
        );
    }
}

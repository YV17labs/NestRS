//! [`ThrottlerConfig`] — rate-limit settings populated from `NESTRS_THROTTLER__*`.

use nest_rs_config::{Config, ConfigService, Result, config};
use validator::Validate;

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

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_env<R>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> R) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for (k, v) in vars {
            match v {
                Some(value) => unsafe { std::env::set_var(k, value) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
        let out = f();
        for (k, _) in vars {
            unsafe { std::env::remove_var(k) };
        }
        out
    }

    fn env_service() -> ConfigService {
        ConfigService::for_namespace("throttler")
    }

    #[test]
    fn defaults_when_no_env_set() {
        with_env(
            &[
                ("NESTRS_THROTTLER__LIMIT", None),
                ("NESTRS_THROTTLER__WINDOW_SECS", None),
                ("NESTRS_THROTTLER__TRUSTED_PROXIES", None),
            ],
            || {
                let cfg = ThrottlerConfig::from_env(&env_service()).expect("no error");
                assert!(cfg.limit.is_none(), "unset ⇒ module default applies later");
                assert!(cfg.window_secs.is_none());
                assert!(cfg.trusted_proxies.is_empty());
            },
        );
    }

    #[test]
    fn from_env_reads_all_fields_when_set() {
        with_env(
            &[
                ("NESTRS_THROTTLER__LIMIT", Some("120")),
                ("NESTRS_THROTTLER__WINDOW_SECS", Some("90")),
                (
                    "NESTRS_THROTTLER__TRUSTED_PROXIES",
                    Some("10.0.0.1,192.168.0.1"),
                ),
            ],
            || {
                let cfg = ThrottlerConfig::from_env(&env_service()).expect("no error");
                assert_eq!(cfg.limit, Some(120));
                assert_eq!(cfg.window_secs, Some(90));
                assert_eq!(
                    cfg.trusted_proxies,
                    vec!["10.0.0.1".to_string(), "192.168.0.1".into()],
                );
            },
        );
    }
}

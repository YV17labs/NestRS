use nest_rs_config::{Config, ConfigService, Result, config};
use validator::Validate;

const DEFAULT_URL: &str = "redis://127.0.0.1/";

#[config(namespace = "queue")]
#[derive(Clone, Debug, Validate)]
pub struct QueueConfig {
    pub url: String,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            url: DEFAULT_URL.to_string(),
        }
    }
}

impl Config for QueueConfig {
    fn from_env(env: &ConfigService) -> Result<Self> {
        Ok(Self {
            url: env.get("URL").unwrap_or_else(|| DEFAULT_URL.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_url_targets_local_loopback_redis() {
        assert_eq!(QueueConfig::default().url, "redis://127.0.0.1/");
    }

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

    #[test]
    fn from_env_falls_back_to_default_url_when_unset() {
        with_env(&[("NESTRS_QUEUE__URL", None)], || {
            let cfg = QueueConfig::from_env(&ConfigService::for_namespace("queue")).expect("ok");
            assert_eq!(cfg.url, DEFAULT_URL);
        });
    }

    #[test]
    fn from_env_picks_up_a_custom_url() {
        with_env(
            &[("NESTRS_QUEUE__URL", Some("redis://redis.staging:6379/2"))],
            || {
                let cfg =
                    QueueConfig::from_env(&ConfigService::for_namespace("queue")).expect("ok");
                assert_eq!(cfg.url, "redis://redis.staging:6379/2");
            },
        );
    }
}

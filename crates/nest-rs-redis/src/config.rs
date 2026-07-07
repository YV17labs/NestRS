use nest_rs_config::{Config, ConfigService, Result, config};
use validator::Validate;

const DEFAULT_URL: &str = "redis://127.0.0.1/";

#[config(namespace = "queue")]
#[derive(Clone, Validate)]
pub struct QueueConfig {
    pub url: String,
}

impl std::fmt::Debug for QueueConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueueConfig")
            .field("url", &"<redacted>")
            .finish()
    }
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

    #[test]
    fn from_env_falls_back_to_default_url_when_unset() {
        let cfg = QueueConfig::from_env(&ConfigService::with_vars("queue", [])).expect("ok");
        assert_eq!(cfg.url, DEFAULT_URL);
    }

    #[test]
    fn from_env_picks_up_a_custom_url() {
        let cfg = QueueConfig::from_env(&ConfigService::with_vars(
            "queue",
            [("NESTRS_QUEUE__URL", "redis://redis.staging:6379/2")],
        ))
        .expect("ok");
        assert_eq!(cfg.url, "redis://redis.staging:6379/2");
    }
}

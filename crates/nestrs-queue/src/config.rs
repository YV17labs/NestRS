use nestrs_config::{config, Config, ConfigService, Result};
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

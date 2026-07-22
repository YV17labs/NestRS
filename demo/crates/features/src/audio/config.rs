use nest_rs_config::{Config, ConfigService, config};
use validator::Validate;

#[config(namespace = "audio")]
#[derive(Clone, Validate)]
pub struct AudioConfig {
    pub synthetic_seed: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            synthetic_seed: true,
        }
    }
}

impl Config for AudioConfig {
    fn from_env(env: &ConfigService) -> nest_rs_config::Result<Self> {
        Ok(Self {
            synthetic_seed: env.parse("SYNTHETIC_SEED")?.unwrap_or(true),
        })
    }
}

use nest_rs::config::{Config, ConfigService, config};

#[config(namespace = "audio")]
#[derive(Clone)]
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
    fn from_env(env: &ConfigService, base: Self) -> nest_rs::config::Result<Self> {
        Ok(Self {
            synthetic_seed: env.parse("SYNTHETIC_SEED")?.unwrap_or(base.synthetic_seed),
        })
    }
}

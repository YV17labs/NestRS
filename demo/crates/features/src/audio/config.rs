use nest_rs::config::{Config, ConfigService, Environment, config};

#[config(namespace = "audio")]
#[derive(Clone, Default)]
pub struct AudioConfig {
    pub synthetic_seed: bool,
}

impl Config for AudioConfig {
    fn defaults() -> Self {
        Self {
            synthetic_seed: !Environment::from_env().is_deployed(),
        }
    }

    fn from_env(env: &ConfigService, base: Self) -> nest_rs::config::Result<Self> {
        Ok(Self {
            synthetic_seed: env.parse("SYNTHETIC_SEED")?.unwrap_or(base.synthetic_seed),
        })
    }
}

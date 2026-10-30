use nest_rs::config::{Environment, config};

#[config(namespace = "audio", defaults = Self::profile_defaults)]
#[derive(Clone, Default)]
pub struct AudioConfig {
    pub synthetic_seed: bool,
}

impl AudioConfig {
    fn profile_defaults() -> Self {
        Self {
            synthetic_seed: !Environment::from_env().is_deployed(),
        }
    }
}

use std::sync::Once;

use nest_rs::config::{Surface, config};

use crate::audio::TARGET;

pub const SEED_OPEN: &str = "synthetic audio seed is open outside a dev profile";

static SEED_OPEN_SAID: Once = Once::new();

#[config(namespace = "audio")]
#[derive(Clone, Default)]
pub struct AudioConfig {
    pub synthetic_seed: Surface,
}

impl AudioConfig {
    pub fn seeds(&self) -> bool {
        if let Some(environment) = self.synthetic_seed.opened_in_deployment() {
            SEED_OPEN_SAID.call_once(|| {
                tracing::warn!(
                    target: TARGET,
                    environment = environment.as_str(),
                    "{SEED_OPEN}",
                );
            });
        }
        self.synthetic_seed.is_open()
    }
}

use nest_rs::config::{Surface, config};

#[config(namespace = "audio")]
#[derive(Clone, Default)]
pub struct AudioConfig {
    pub synthetic_seed: Surface,
}

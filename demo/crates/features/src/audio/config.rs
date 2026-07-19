use nest_rs_config::{Config, ConfigService, config};
use validator::Validate;

/// Audio feature knobs, loaded from `NESTRS_AUDIO__*` through the standard
/// dual-path config seam (env cascade + pinned struct).
#[config(namespace = "audio")]
#[derive(Clone, Validate)]
pub struct AudioConfig {
    /// Whether the `#[every("5s")]` schedule seeds a synthetic source object
    /// and enqueues its transcode. On by default so the demo shows the queue
    /// working out of the box; turn off (`NESTRS_AUDIO__SYNTHETIC_SEED=false`)
    /// for a long-running deployment — every fire writes an object to
    /// storage, which would grow the bucket forever.
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

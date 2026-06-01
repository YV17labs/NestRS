//! [`QueueConfig`] — the Redis connection settings for [`QueueModule`], a
//! namespaced `#[config]` loaded from `NESTRS_QUEUE__*` (and the `.env` cascade).

use nestrs_config::config;
use serde::Deserialize;
use validator::Validate;

const DEFAULT_URL: &str = "redis://127.0.0.1/";

fn default_url() -> String {
    DEFAULT_URL.to_string()
}

#[config(namespace = "queue")]
#[derive(Clone, Debug, Deserialize, Validate)]
pub struct QueueConfig {
    /// The Redis URL backing the queues (`NESTRS_QUEUE__URL`). Defaults to a
    /// local Redis when unset.
    #[serde(default = "default_url")]
    pub url: String,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self { url: default_url() }
    }
}

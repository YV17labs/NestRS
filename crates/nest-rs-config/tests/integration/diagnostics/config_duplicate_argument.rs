//! A `#[config]` key written twice is refused rather than resolved by source
//! order.
//!
//! The dropped declaration here is the namespace — which decides every
//! `<PREFIX>_<NS>__*` variable the struct reads, so the silent arm is a whole
//! config reading a deployment's other domain.

use nest_rs_config::config;

#[config(namespace = "storage", namespace = "database")]
#[derive(Clone, Debug, Default)]
struct StorageConfig {
    url: String,
}

fn main() {}

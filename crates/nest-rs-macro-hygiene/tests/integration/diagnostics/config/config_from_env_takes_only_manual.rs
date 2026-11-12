//! `from_env` has one value, `"manual"`: the overlay is derived unless the
//! struct writes it, so any other value is refused.

use nest_rs::config::config;

#[config(namespace = "kiln", from_env = "derived")]
#[derive(Clone, Default)]
struct KilnConfig {
    port: u16,
}

fn main() {}

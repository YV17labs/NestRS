//! `defaults` is the derived `impl Config`'s: under `from_env = "manual"` the
//! struct's own impl writes `fn defaults()`, so the key is refused, pointing at
//! both.

use nest_rs::config::config;

#[config(namespace = "kiln", from_env = "manual", defaults = Self::for_profile)]
#[derive(Clone, Default)]
struct KilnConfig {
    port: u16,
}

fn main() {}

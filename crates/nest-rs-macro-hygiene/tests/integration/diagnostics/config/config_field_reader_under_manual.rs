//! Under `from_env = "manual"` the struct's own overlay reads every field, so
//! a field saying how the derived one would read it reaches nothing.

use nest_rs::config::{Config, ConfigService, config};

#[config(namespace = "kiln", from_env = "manual")]
#[derive(Clone, Default)]
struct KilnConfig {
    #[config(json)]
    shelves: Vec<String>,
}

impl Config for KilnConfig {
    fn from_env(_: &ConfigService, base: Self) -> nest_rs::config::Result<Self> {
        Ok(base)
    }
}

fn main() {}

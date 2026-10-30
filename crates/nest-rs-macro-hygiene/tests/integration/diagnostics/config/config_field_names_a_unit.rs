//! A field's name is its key, and a duration's unit is its bounds' to carry: a
//! field named for a unit is refused, naming the field it should be.

use nest_rs::config::config;

#[config(namespace = "kiln")]
#[derive(Clone, Default)]
struct KilnConfig {
    window_secs: u64,
}

fn main() {}

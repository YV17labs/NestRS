//! A field's name is its key, and a key holds no `__`, the namespace's
//! separator: a field named with one is refused, naming it.

use nest_rs::config::config;

#[config(namespace = "kiln")]
#[derive(Clone, Default)]
struct KilnConfig {
    queue__url: String,
}

fn main() {}

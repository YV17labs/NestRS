//! A field is read one way: a second reader on it is refused, naming the
//! first.

use nest_rs::config::config;

#[config(namespace = "kiln")]
#[derive(Clone, Default)]
struct KilnConfig {
    #[config(json, count)]
    batches: Option<usize>,
}

fn main() {}

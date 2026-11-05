//! `#[config(count)]` is gone: a count reads its range, and the refusal says so.

use nest_rs::config::config;

#[config(namespace = "kiln")]
#[derive(Clone, Default)]
struct KilnConfig {
    #[config(count)]
    batches: Option<usize>,
}

fn main() {}

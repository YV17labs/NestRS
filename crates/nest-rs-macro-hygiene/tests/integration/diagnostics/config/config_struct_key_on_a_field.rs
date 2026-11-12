//! A struct's key written on a field is not misspelled: it is told whose it
//! is, rather than listed as unknown.

use nest_rs::config::config;

#[config(namespace = "kiln")]
#[derive(Clone, Default)]
struct KilnConfig {
    #[config(from_env = "manual")]
    port: u16,
}

fn main() {}

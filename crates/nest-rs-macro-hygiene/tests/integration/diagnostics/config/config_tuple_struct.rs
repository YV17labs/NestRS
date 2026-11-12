//! Each field is read under its name, and a tuple struct's fields have none.

use nest_rs::config::config;

#[config(namespace = "kiln")]
#[derive(Clone, Default)]
struct KilnConfig(u16);

fn main() {}

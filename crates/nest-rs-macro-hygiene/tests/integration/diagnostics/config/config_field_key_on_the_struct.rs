//! A field's key written on the struct is not misspelled: it is told to sit on
//! the field it reads.

use nest_rs::config::config;

#[config(namespace = "kiln", json)]
#[derive(Clone, Default)]
struct KilnConfig {
    shelves: Vec<String>,
}

fn main() {}

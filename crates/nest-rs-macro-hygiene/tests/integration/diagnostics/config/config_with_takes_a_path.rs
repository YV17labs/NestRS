//! `with` names the function reading the field, so anything but a path is
//! refused with the shape that function takes.

use nest_rs::config::config;

#[config(namespace = "kiln")]
#[derive(Clone, Default)]
struct KilnConfig {
    #[config(with = "doubled")]
    heat: u32,
}

fn main() {}

//! `defaults` names the function building the profile-aware base, so anything
//! but a path is refused with the shape that function takes.

use nest_rs::config::config;

#[config(namespace = "kiln", defaults = "for_profile")]
#[derive(Clone, Default)]
struct KilnConfig {
    port: u16,
}

fn main() {}

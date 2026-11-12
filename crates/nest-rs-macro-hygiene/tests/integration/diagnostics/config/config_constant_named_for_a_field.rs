//! `#[config]` binds every field to the associated constant named for it, so a
//! constant of the struct's own under that name is refused at both.

use nest_rs::config::config;

#[config(namespace = "kiln")]
#[derive(Clone, Default)]
pub struct KilnConfig {
    pub port: u16,
}

impl KilnConfig {
    pub const PORT: u16 = 3000;
}

fn main() {}

//! `Unnamed` is the default instance's marker, which adds no namespace segment;
//! a second marker of that name would read the default's variables.

use nest_rs::core::instance;

#[instance]
pub struct Unnamed;

fn main() {}

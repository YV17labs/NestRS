//! `#[output]` derives `Serialize` and `JsonSchema`, which describe a value an
//! edge sends: a struct or an enum. A union is refused by that fact, naming it.

use nest_rs::core::output;

#[output]
pub union Reading {
    raw: u32,
    scaled: f32,
}

fn main() {}

//! One decorator, one item shape: `#[wire_enum]` carries the wire derives for an
//! enum, so on a struct it names the struct's decorators rather than reporting
//! syn's `expected enum`.

use nest_rs::core::wire_enum;

#[wire_enum]
pub struct Model {
    pub id: u32,
}

fn main() {}

//! One decorator, one item shape: `#[wire_enum]` carries the wire derives for a
//! column's enum *type*, so on the entity struct it points back at `#[expose]`
//! rather than reporting syn's `expected enum`.

use nest_rs_resource::wire_enum;

#[wire_enum]
pub struct Model {
    pub id: u32,
}

fn main() {}

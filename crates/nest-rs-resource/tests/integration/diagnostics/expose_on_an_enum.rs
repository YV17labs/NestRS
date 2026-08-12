//! One decorator, one item shape — the mirror of `wire_enum_on_a_struct`.
//! "Make this reach the wire" is one intent with two item shapes, so the
//! refusal names the sibling that takes the other one instead of leaving syn to
//! say `expected struct`.

use nest_rs_resource::expose;

#[expose(name = "Tier")]
pub enum Tier {
    Free,
    Pro,
}

fn main() {}

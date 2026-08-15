//! The impl half collects; it declares nothing. The sentence is
//! `nest_rs_codegen::pair`'s, so every pair says it the same way — this one
//! wrote its own until the shapes join showed which pairs had a fixture and
//! which had only the behaviour.

use nest_rs_events::listeners;

struct DemoProvider;

#[listeners(async_mode = true)]
impl DemoProvider {}

fn main() {}

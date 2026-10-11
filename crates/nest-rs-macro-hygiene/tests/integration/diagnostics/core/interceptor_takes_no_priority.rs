//! The HTTP attribute's `priority` key is refused by name: an interceptor
//! orders itself within its stage through its `Layer` impl.

use nest_rs::core::interceptor;

#[interceptor(priority = 1)]
#[derive(Default)]
struct Prioritised;

fn main() {}

//! `#[interceptor(stage = unit)]` on a type implementing no `Interceptor` names the trait
//! and what to implement, never rustc's bare unsatisfied bound.

use nest_rs::core::{Layer, interceptor};

#[interceptor(stage = unit)]
#[derive(Default, Layer)]
struct Forgetful;

fn main() {}

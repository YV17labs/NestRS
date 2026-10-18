//! `#[interceptor]` on a type implementing no `Interceptor` names the trait
//! and what to implement, never rustc's bare unsatisfied bound.

use nest_rs::core::{Layer, interceptor};

#[interceptor]
#[derive(Default)]
struct Forgetful;

impl Layer for Forgetful {}

fn main() {}

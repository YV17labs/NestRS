//! Listing a type implementing no `ExceptionFilter` as one names the trait and
//! what to implement, never rustc's bare unsatisfied bound.

use nest_rs::core::{Layer, exception_filter};

#[derive(Default)]
struct Forgetful;

impl Layer for Forgetful {}

fn main() {
    let _ = exception_filter::<Forgetful>();
}

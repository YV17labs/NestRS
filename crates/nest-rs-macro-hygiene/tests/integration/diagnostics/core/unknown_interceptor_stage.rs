//! `#[interceptor(stage = …)]` accepts only `unit` — a stage outside it is a
//! spanned compile error, not a silent default.

use nest_rs::core::interceptor;

#[interceptor(stage = sideways)]
#[derive(Default)]
struct Sideways;

fn main() {}

//! The same fact, at the shape that used to slip past: a tuple struct reached
//! the derives and was refused *inside* the expansion by `validator`, pointing
//! at a `#[derive(...)]` line the developer never wrote.

use nest_rs_core::input;

#[input]
pub struct Slug(pub String);

fn main() {}

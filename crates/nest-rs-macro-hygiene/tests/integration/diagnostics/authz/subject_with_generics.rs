//! A rule and a posture name one class, by its type: a generic marker would be
//! several.

use nest_rs::authz::subject;

#[subject]
pub struct Exporting<T>;

fn main() {}

//! `#[subject]` takes no argument: a subject is named by its type.

use nest_rs::authz::subject;

#[subject(name = "exporting")]
pub struct Exporting;

fn main() {}

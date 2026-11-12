//! A subject names a class and holds nothing: a rule grants the whole class,
//! and an entity is the subject whose rows a rule narrows.

use nest_rs::authz::subject;

#[subject]
pub struct Exporting {
    pub org: u32,
}

fn main() {}

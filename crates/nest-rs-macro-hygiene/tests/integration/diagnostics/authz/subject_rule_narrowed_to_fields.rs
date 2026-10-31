//! `fields` narrows the columns an entity's grant reaches; a subject no table
//! holds has none, so the call is refused saying so.

use nest_rs::authz::{AbilityBuilder, Action, subject};

#[subject]
pub struct Exporting;

fn main() {
    let mut ab = AbilityBuilder::new();
    ab.can(Action::Manage, Exporting).fields(Vec::new());
}

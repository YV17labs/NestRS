//! `when` narrows the rows an entity's rule reaches; a subject no table holds
//! has no rows, so the method is not there to call.

use nest_rs::authz::{AbilityBuilder, Action, subject};

#[subject]
pub struct Exporting;

fn main() {
    let mut ab = AbilityBuilder::new();
    ab.can(Action::Manage, Exporting).when(|_| unreachable!());
}

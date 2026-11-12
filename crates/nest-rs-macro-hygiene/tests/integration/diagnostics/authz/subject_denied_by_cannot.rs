//! A denial narrows the rows a grant reaches, and a subject no table holds has
//! none: its class gate is its one layer, so a grant not written is its denial.

use nest_rs::authz::{AbilityBuilder, Action, subject};

#[subject]
pub struct Exporting;

fn main() {
    let mut ab = AbilityBuilder::new();
    ab.cannot(Action::Manage, Exporting);
}

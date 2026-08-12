//! An empty enum has no value to serialize and no GraphQL enum to name, so the
//! derives it would carry are all vacuous.

use nest_rs_resource::wire_enum;

#[wire_enum]
pub enum Tier {}

fn main() {}

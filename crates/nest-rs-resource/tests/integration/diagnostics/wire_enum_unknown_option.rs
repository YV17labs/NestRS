//! `graphql` is the only option, and it is the one that decides whether an
//! `Enum` derive lands on the type — a typo silently means "HTTP only".

use nest_rs_resource::wire_enum;

#[wire_enum(gql)]
pub enum Tier {
    Free,
    Pro,
}

fn main() {}

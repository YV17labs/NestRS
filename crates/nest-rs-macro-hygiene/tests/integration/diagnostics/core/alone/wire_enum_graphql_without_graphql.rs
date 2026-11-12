//! `#[wire_enum(graphql)]` in a build without `graphql` names the feature to add
//! and the attribute the developer wrote, where rustc said `E0433` alone. It
//! deliberately fails to resolve: the derive names `nest_rs::graphql`, which the
//! build does not enable, and rustc reports that path beside the refusal.

use nest_rs::core::wire_enum;

#[wire_enum(graphql)]
pub enum Stage {
    Draft,
    Shipped,
}

fn main() {}

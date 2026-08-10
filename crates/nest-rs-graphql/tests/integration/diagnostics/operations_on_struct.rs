//! The converse: `#[operations]` collects a resolver's methods, so it has
//! nothing to say about a struct — and the error says which decorator does.

use nest_rs_graphql::operations;

#[operations]
struct DemoResolver;

fn main() {}

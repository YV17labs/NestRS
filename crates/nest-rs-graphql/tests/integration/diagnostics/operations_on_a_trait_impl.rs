//! One decorator, one item shape — the third shape, and the one that was taken
//! in silence: a trait impl parses as an `impl` like any other, so `#[operations]`
//! was accepted and collected nothing. The operation declared here would simply
//! not exist, and no diagnostic anywhere would say so.

use nest_rs_graphql::operations;

struct DemoResolver;

#[operations]
impl Default for DemoResolver {
    fn default() -> Self {
        Self
    }
}

fn main() {}

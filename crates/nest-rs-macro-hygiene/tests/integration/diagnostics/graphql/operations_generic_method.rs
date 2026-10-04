//! An operation is called with the arguments a query sends and nothing else, so
//! a type parameter has nothing to be inferred from.

use nest_rs::graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[public]
    async fn widget<const N: usize>(&self) -> i32 {
        0
    }
}

fn main() {}

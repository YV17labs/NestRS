//! An `#[operations]` method borrows its resolver: `&mut self` is refused with
//! that fact, one sentence for every impl half, rather than a mutability error
//! against the root object the expansion writes.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[public]
    async fn widget(&mut self, id: i32) -> i32 {
        id
    }
}

fn main() {}

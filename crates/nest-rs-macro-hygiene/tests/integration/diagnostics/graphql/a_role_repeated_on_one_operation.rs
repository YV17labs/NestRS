//! One role written again is refused counting every copy the method carries,
//! rather than naming the first two attributes as though they were two roles.

use nest_rs::graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[query]
    #[query]
    #[public]
    async fn widget(&self, id: i32) -> i32 {
        id
    }
}

fn main() {}

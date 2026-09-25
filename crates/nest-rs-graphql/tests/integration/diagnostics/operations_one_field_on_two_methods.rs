//! A root resolves each field to one method. Two methods the schema calls by one
//! name are refused: async-graphql keeps the second in the SDL and dispatches to
//! the first, so the arguments a client reads are not the body that runs.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[public]
    async fn widget(&self) -> i32 {
        1
    }

    #[query]
    #[public]
    #[graphql(name = "widget")]
    async fn other_widget(&self, id: i32) -> i32 {
        id
    }
}

fn main() {}

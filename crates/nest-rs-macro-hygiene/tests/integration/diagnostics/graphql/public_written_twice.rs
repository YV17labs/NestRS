//! A decorator read once is refused by name when written twice. The first copy
//! was taken and the second left on the method, where rustc reported
//! `cannot find attribute` — no decorator, no reason, no remedy. One sentence
//! for every such attribute (`nest_rs::codegen::take_single_attr`), pinned at
//! each edge.

use nest_rs::graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[public]
    #[public]
    async fn ping(&self) -> i32 {
        0
    }
}

fn main() {}

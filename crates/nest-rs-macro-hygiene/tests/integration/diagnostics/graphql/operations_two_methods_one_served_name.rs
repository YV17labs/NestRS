//! Two methods async-graphql's naming rule serves under one field — `a1_b` and
//! `a_1b` are both `a1B` — are refused by name. Through 6.x both compiled and one
//! method's body ran for the other's field; the framework now names every field
//! by async-graphql's own rule, so the duplicate check reads the name served.

use nest_rs::graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[public]
    async fn a1_b(&self, left: i32) -> i32 {
        left
    }

    #[query]
    #[public]
    async fn a_1b(&self, right: i32) -> i32 {
        right
    }
}

fn main() {}

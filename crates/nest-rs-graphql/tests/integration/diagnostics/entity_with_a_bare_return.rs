//! The guard chain is only emitted for a `Result`-returning operation — a
//! bare-return body has nowhere to put a denial. On a `#[query]` that trade is
//! visible in the document; an entity is reached for a type the client never
//! named, so a resolver-scope `#[use_guards]` compiled out here shows up
//! nowhere at all.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> i32 {
        id
    }
}

fn main() {}

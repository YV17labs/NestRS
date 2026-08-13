//! The seventh refusal. `_entities` resolves a batch of references
//! concurrently and async-graphql awaits the entity resolver, so a synchronous
//! body has nowhere to be awaited — the same reading `#[subscription]` takes,
//! and it carries its own snapshot for the same reason.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[entity]
    #[public]
    fn find_widget_by_id(&self, id: i32) -> nest_rs_graphql::async_graphql::Result<i32> {
        Ok(id)
    }
}

fn main() {}

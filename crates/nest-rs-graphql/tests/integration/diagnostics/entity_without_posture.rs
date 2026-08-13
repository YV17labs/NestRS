//! An `#[entity]` with neither `#[authorize(...)]` nor `#[public]` must not
//! compile, and the sentence has to say why this role is the worst one to
//! forget: the router resolves it from a reference the client never wrote, so
//! an ungated entity is readable from outside every gate in the schema.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[entity]
    async fn find_widget_by_id(&self, id: i32) -> nest_rs_graphql::async_graphql::Result<i32> {
        Ok(id)
    }
}

fn main() {}

//! async-graphql's derive reads the **first** `graphql` attribute on a method
//! and removes exactly one. `#[entity]` has to emit `#[graphql(entity)]` there,
//! so a developer's own would silently take its place: the method stops being an
//! entity resolver, and what the compiler reports is a leftover attribute
//! against `#[operations]`. There is no working spelling to redirect to.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[entity]
    #[graphql(name = "findWidget")]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> nest_rs_graphql::async_graphql::Result<i32> {
        Ok(id)
    }
}

fn main() {}

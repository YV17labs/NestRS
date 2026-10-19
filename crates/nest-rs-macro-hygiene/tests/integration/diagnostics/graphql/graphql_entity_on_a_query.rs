//! async-graphql's own `#[graphql(entity)]` keys the returned type, and a keyed
//! schema serves `_service` and `_entities` whatever `federation` says. Beside
//! `#[query]` it would key outside the posture, the federation fields' guard
//! pool and the boot's subgraph check that `#[entity]` carries.

use nest_rs::graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[public]
    #[graphql(entity)]
    async fn find_widget_by_id(&self, id: i32) -> nest_rs::graphql::async_graphql::Result<i32> {
        Ok(id)
    }
}

fn main() {}

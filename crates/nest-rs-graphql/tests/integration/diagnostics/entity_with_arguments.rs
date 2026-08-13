//! The `@key` is inferred from the resolver's own arguments, so `#[entity]`
//! takes none of its own. `key = "…"` is the first thing a developer arriving
//! from Apollo reaches for, and accepting it silently would be the ignored
//! argument the rules call silence.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[entity(key = "id")]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> nest_rs_graphql::async_graphql::Result<i32> {
        Ok(id)
    }
}

fn main() {}

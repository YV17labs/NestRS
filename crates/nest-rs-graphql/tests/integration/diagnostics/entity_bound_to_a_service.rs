//! `bind = Service` answers `NOT_FOUND` for an absent row and `FORBIDDEN` for a
//! withheld one. On a field the router addresses **by key**, that pair is an
//! existence oracle — right for a mutation subject the caller already named,
//! wrong for a reference.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[entity]
    #[authorize(Read, bind = WidgetsService)]
    async fn find_widget_by_id(&self, id: i32) -> nest_rs_graphql::async_graphql::Result<i32> {
        Ok(id)
    }
}

fn main() {}

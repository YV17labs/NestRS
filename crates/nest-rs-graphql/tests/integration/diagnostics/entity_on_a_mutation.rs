//! `#[entity]` names a role, not a modifier: an entity is resolved through the
//! `Query` root's `_entities` field, which no other root has. Combining it with
//! another verb must name both rather than silently mounting one.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[mutation]
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> i32 {
        id
    }
}

fn main() {}

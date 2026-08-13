//! The same refusal at the other root that has no `_entities`. Both are pinned:
//! the sentence is one helper's, and a snapshot per site is what keeps it from
//! drifting into two answers.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[subscription]
    #[entity]
    #[public]
    async fn find_widget_by_id(&self, id: i32) -> impl futures_util::Stream<Item = i32> {
        futures_util::stream::iter([id])
    }
}

fn main() {}

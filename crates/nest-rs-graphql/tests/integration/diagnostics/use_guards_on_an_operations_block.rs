//! The GraphQL member of the host-scope-layer family — one sentence, four
//! edges. This edge worded it itself and named two siblings where the
//! uniformity spans four, so a resolver author was told the wrong thing.

use nest_rs_graphql::{operations, resolver};

struct AllowAll;

#[resolver]
#[derive(Default)]
struct WidgetsResolver;

#[operations]
#[use_guards(AllowAll)]
impl WidgetsResolver {
    #[query]
    #[public]
    async fn widgets(&self) -> String {
        String::new()
    }
}

fn main() {}

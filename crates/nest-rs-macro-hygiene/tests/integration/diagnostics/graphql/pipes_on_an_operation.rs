//! `#[use_pipes]` on a GraphQL operation is refused for good, naming the
//! per-argument pipe: the global pipes fold an operation's variables once per
//! request, so no pipe is scoped to one field.

use nest_rs::graphql::{operations, resolver};

struct Trim;

#[resolver]
#[derive(Default)]
struct WidgetsResolver;

#[operations]
impl WidgetsResolver {
    #[query]
    #[public]
    #[use_pipes(Trim)]
    async fn widget(&self, name: String) -> String {
        name
    }
}

fn main() {}

//! Two fields of one root under `#[cfg]`s that both hold are two resolvers for
//! one name — refused by rustc, which is what evaluates the conditions.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[cfg(all())]
    #[query]
    #[public]
    async fn widget(&self) -> i32 {
        1
    }

    #[cfg(all())]
    #[query]
    #[public]
    #[graphql(name = "widget")]
    async fn other_widget(&self, id: i32) -> i32 {
        id
    }
}

fn main() {}

//! A `&DataLoader` naming no loader on a root field is no loader parameter, so
//! the refusal is the compiler's own about the missing generic.

use nest_rs::graphql::async_graphql::dataloader::DataLoader;
use nest_rs::graphql::{operations, resolver};

#[resolver]
struct NoteResolver;

#[operations]
impl NoteResolver {
    #[query]
    #[public]
    async fn label(&self, labels: &DataLoader) -> String {
        String::new()
    }
}

fn main() {}

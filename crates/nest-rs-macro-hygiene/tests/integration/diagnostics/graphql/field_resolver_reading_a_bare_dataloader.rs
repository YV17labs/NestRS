//! A `&DataLoader` naming no loader reads as any other reference, so the refusal
//! is rustc's, with no sentence of the decorator's and nothing cascading on the
//! parent.

use nest_rs::graphql::async_graphql::SimpleObject;
use nest_rs::graphql::async_graphql::dataloader::DataLoader;
use nest_rs::graphql::{operations, resolver};

#[derive(SimpleObject)]
#[graphql(crate = "::nest_rs::graphql::async_graphql", complex)]
struct Note {
    id: i32,
}

#[resolver]
struct NoteResolver;

#[operations]
impl NoteResolver {
    #[query]
    #[public]
    async fn note(&self) -> Note {
        Note { id: 1 }
    }

    #[field_resolver]
    async fn label(&self, parent: &Note, labels: &DataLoader) -> String {
        let _ = labels;
        parent.id.to_string()
    }
}

fn main() {}

//! A `&DataLoader<L>` parameter declares a need the boot checks through the
//! loader's owner, which only a loader `#[dataloader]` generated says.

use std::collections::HashMap;
use std::convert::Infallible;

use nest_rs::graphql::async_graphql::SimpleObject;
use nest_rs::graphql::async_graphql::dataloader::{DataLoader, Loader};
use nest_rs::graphql::{operations, resolver};

struct HandWritten;

impl Loader<i32> for HandWritten {
    type Value = String;
    type Error = Infallible;

    async fn load(&self, _: &[i32]) -> Result<HashMap<i32, String>, Infallible> {
        Ok(HashMap::new())
    }
}

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
    async fn label(&self, parent: &Note, labels: &DataLoader<HandWritten>) -> String {
        let Ok(found) = labels.load_one(parent.id).await;
        found.unwrap_or_default()
    }
}

fn main() {}

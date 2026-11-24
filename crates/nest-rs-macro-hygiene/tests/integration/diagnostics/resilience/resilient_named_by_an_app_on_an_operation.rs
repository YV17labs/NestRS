//! An attribute of the app's own, imported under the name of the resilience
//! capability's decorator, on an operation answering no `Result`:
//! `#[operations]` reads the name, so it holds the operation to what a
//! decorated one answers. Its one sentence says so, and what an app whose
//! attribute is no decorator of the framework's writes instead.

use nest_rs::graphql::{operations, resolver};
// The app's own attribute macro, named as the decorator is.
use tracing::instrument as resilient;

#[resolver]
struct EntryResolver;

#[operations]
impl EntryResolver {
    #[query]
    #[public]
    #[resilient(skip_all)]
    async fn entry(&self, id: i32) -> String {
        format!("entry {id}")
    }
}

fn main() {}

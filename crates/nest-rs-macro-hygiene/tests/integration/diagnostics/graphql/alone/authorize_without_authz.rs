//! `#[authorize]` in a build without `authz` names the feature to add, where
//! rustc said `E0433` alone. It deliberately fails to resolve: the expansion
//! names `nest_rs::authz`, which the build does not enable.

use nest_rs::graphql::{operations, resolver};

struct Read;
struct Note;

#[resolver]
struct NotesResolver;

#[operations]
impl NotesResolver {
    #[query]
    #[authorize(Read, Note)]
    async fn note_count(&self) -> i32 {
        0
    }
}

fn main() {}

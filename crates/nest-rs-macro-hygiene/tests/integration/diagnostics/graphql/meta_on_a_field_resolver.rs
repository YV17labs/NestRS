//! A `#[field_resolver]` runs under the root field that produced its parent and
//! is no unit of its own, so it declares no `#[meta]`.

use nest_rs::graphql::{operations, resolver};

struct Limit(u32);

struct Note {
    body: String,
}

#[resolver]
struct NoteResolver;

#[operations]
impl NoteResolver {
    #[query]
    #[public]
    async fn note(&self) -> String {
        String::new()
    }

    #[field_resolver]
    #[meta(Limit(10))]
    async fn shout(&self, parent: &Note) -> String {
        parent.body.to_uppercase()
    }
}

fn main() {}

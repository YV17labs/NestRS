//! Two `#[meta]` values of one type on one root field: refused at the second.

use nest_rs::graphql::{operations, resolver};

struct Limit(u32);

#[resolver]
struct NoteResolver;

#[operations]
impl NoteResolver {
    #[query]
    #[public]
    #[meta(Limit(10))]
    #[meta(Limit(20))]
    async fn note(&self) -> String {
        String::new()
    }
}

fn main() {}

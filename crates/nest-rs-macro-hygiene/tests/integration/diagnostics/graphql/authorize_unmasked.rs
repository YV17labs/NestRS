//! `unmasked` is refused **by name** on every edge, with the fact that makes it
//! meaningless: no reply mask is left to switch off, since each wire row a reply
//! carries was judged for its caller where `#[expose]`'s conversion built it.

use nest_rs::graphql::{operations, resolver};

struct Read;
mod users {
    pub struct Entity;
}

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[authorize(Read, users::Entity, unmasked)]
    async fn ping(&self) -> i32 {
        0
    }
}

fn main() {}

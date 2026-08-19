//! One of the four refusals a `key = value` grammar owes, pinned where the
//! compiler says it. `CLAUDE.md`: "Refusals are shared, not per key. One
//! helper, one sentence, every key it covers, **one trybuild snapshot per
//! site**."

use nest_rs_graphql::{operations, resolver};

struct Update;
struct ArtworksService;
struct PostsService;

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[mutation]
    #[authorize(Update, bind = ArtworksService, bind = PostsService)]
    async fn touch(&self) -> i32 {
        0
    }
}

fn main() {}

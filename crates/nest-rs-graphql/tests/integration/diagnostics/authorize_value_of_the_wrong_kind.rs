//! `#[authorize]`'s values, each of the wrong kind and each refused at itself: a
//! `bind` that is not a service path and an `id_arg` that is not an identifier
//! open with the decorator and the key, and a positional that is not a path gets
//! the attribute's shape sentence, as on the other three edges. syn's
//! `expected identifier` named none of them.

use nest_rs_graphql::{operations, resolver};

struct Update;
struct Read;
struct ArtworksService;

#[resolver]
struct AResolver;

#[operations]
impl AResolver {
    #[mutation]
    #[authorize(Update, bind = "ArtworksService")]
    async fn touch(&self) -> i32 {
        0
    }
}

#[resolver]
struct BResolver;

#[operations]
impl BResolver {
    #[mutation]
    #[authorize(Update, bind = ArtworksService, id_arg = "file_id")]
    async fn touch(&self) -> i32 {
        0
    }
}

#[resolver]
struct CResolver;

#[operations]
impl CResolver {
    #[query]
    #[authorize(Read, "users")]
    async fn list(&self) -> i32 {
        0
    }
}

fn main() {}

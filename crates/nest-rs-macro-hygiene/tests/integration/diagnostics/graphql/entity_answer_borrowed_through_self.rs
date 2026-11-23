//! An `#[entity]` claims the type it resolves by that type's name, read
//! outside the method, so its answer borrows nothing through the method's
//! lifetime: `&'a Post` borrowed through `&'a self` is rustc's `E0261`, the
//! lifetime deliberately failing to resolve where the claim reads the answer.
//! A query answers such a borrow.

use nest_rs::graphql::async_graphql::SimpleObject;
use nest_rs::graphql::{operations, resolver};

#[derive(Default, SimpleObject)]
struct Post {
    id: i32,
}

#[resolver]
#[derive(Default)]
struct PostsResolver {
    post: Post,
}

#[operations]
impl PostsResolver {
    #[entity]
    #[public]
    async fn find_post_by_id<'a>(&'a self, id: i32) -> &'a Post {
        let _ = id;
        &self.post
    }
}

fn main() {}

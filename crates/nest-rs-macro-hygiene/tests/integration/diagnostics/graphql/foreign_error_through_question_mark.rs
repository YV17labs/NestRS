//! A resolver body may not `?` a foreign error into `async_graphql::Result`:
//! the conversion would ship the error's `Display` to the client before the
//! wrapper could answer it opaquely, so async-graphql's blanket conversion is
//! off and the resolver returns the error itself.

use nest_rs::graphql::async_graphql::Result;
use nest_rs::graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[public]
    async fn ping(&self) -> Result<i32> {
        let read: std::io::Result<i32> = Err(std::io::Error::other("secret"));
        Ok(read?)
    }
}

fn main() {}

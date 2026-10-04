//! A `Result` under another name around a stream type that *can* be named
//! reaches the type checker, and is known there by its type: a subscription
//! answers a stream, so it is refused at the return type with the fix. The
//! derive's own errors follow it — async-graphql takes the `Result` for the
//! stream — and are pinned so a change in the first line is seen.

use nest_rs::graphql::async_graphql::futures_util::stream::{self, BoxStream, StreamExt};
use nest_rs::graphql::{operations, resolver};

type Fallible<T> = nest_rs::graphql::async_graphql::Result<T>;

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[subscription]
    #[public]
    async fn ticks(&self) -> Fallible<BoxStream<'static, i32>> {
        Ok(stream::iter(0..3).boxed())
    }
}

fn main() {}

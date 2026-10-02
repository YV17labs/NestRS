//! async-graphql decides "is this fallible?" from the **spelling** of the
//! return type's last path segment, and its subscription derive builds paths
//! out of that type — where Rust refuses an `impl Trait`. So a stream inside a
//! `Result` under another name, the `use async_graphql::Result as GqlResult`
//! idiom, cannot reach it. The decorator says so at the return type rather than
//! leaving the derive to emit a wall of errors — read off where the `impl`
//! sits, never off the alias's name.

// The failed expansion leaves the method out, so these read as unused.
#[allow(unused_imports)]
use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream};
use nest_rs_graphql::{operations, resolver};

type GqlResult<T> = nest_rs_graphql::async_graphql::Result<T>;

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[subscription]
    #[public]
    async fn ticks(&self) -> GqlResult<impl Stream<Item = i32>> {
        Ok(stream::iter(0..3))
    }
}

fn main() {}

//! async-graphql decides "is this fallible?" from the **spelling** of the
//! return type's last path segment, so an aliased `Result` is read as an
//! ordinary value and the stream type becomes the `Result` itself. The
//! decorator names that rule instead of letting the derive emit a wall of
//! trait errors.

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

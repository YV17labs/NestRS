//! A `#[subscription]` with neither `#[authorize(...)]` nor `#[public]` must
//! not compile — deny-all by default, exactly as for a `#[query]`. A
//! subscription is the operation where an omission costs the most: it keeps
//! pushing.

// The failed expansion leaves the method out, so these read as unused.
#[allow(unused_imports)]
use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream};
use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[subscription]
    async fn ticks(&self) -> impl Stream<Item = i32> {
        stream::iter(0..3)
    }
}

fn main() {}

//! A `#[subscription]` method is awaited once to produce the stream, so a
//! synchronous one cannot exist. The error names that rule on the method rather
//! than surfacing async-graphql's own message from inside the expansion.

// The failed expansion leaves the method out, so these read as unused.
#[allow(unused_imports)]
use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream};
use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[subscription]
    #[public]
    fn ticks(&self) -> impl Stream<Item = i32> {
        stream::iter(0..3)
    }
}

fn main() {}

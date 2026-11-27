//! Under a posture naming a subject a subscription's stream is filtered by
//! item, so a stream a type names is read for what it yields. A type that is
//! no stream — a `Result` under another name around one — is told so first,
//! and the ungated refusal's sentence follows at the return type, by kind.

use nest_rs::authz::{Read, subject};
use nest_rs::graphql::async_graphql::futures_util::stream::{self, BoxStream, StreamExt};
use nest_rs::graphql::{operations, resolver};

#[subject]
struct Exports;

type Fallible<T> = nest_rs::graphql::async_graphql::Result<T>;

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[subscription]
    #[authorize(Read, Exports)]
    async fn ticks(&self) -> Fallible<BoxStream<'static, i32>> {
        Ok(stream::iter(0..3).boxed())
    }
}

fn main() {}

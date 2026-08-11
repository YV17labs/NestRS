//! `#[resolver]` + `#[operations]` — the GraphQL operation surface,
//! witnessed through the umbrella alone.
//!
//! What this proves that nothing else does: **`#[operations]` wraps
//! async-graphql's own `#[Object]`**, whose expansion roots its paths at
//! whatever `proc-macro-crate` finds in the *call site's* manifest — falling
//! back to a bare `::async_graphql` when it finds nothing. That fallback made
//! the first snippet of `/graphql/` uncompilable for anyone who installed the
//! documented `nest-rs = { features = ["graphql"] }` and nothing else. The
//! `crate = ` override `#[operations]` now threads through is what pins the
//! expansion to the umbrella's re-export, and this module is the compile that
//! fails if it is ever dropped.

//! `#[subscription]` widens that same claim: its expansion reaches
//! async-graphql's `#[Subscription]` derive **and** `futures_util`'s
//! `StreamExt` — two crates the developer's own source never names. Both are
//! reached through the umbrella's re-export, so the manifest below stays at one
//! line.

use nest_rs::graphql::async_graphql::futures_util::stream::{self, Stream};
use nest_rs::graphql::{operations, resolver};

/// The lead snippet of `/graphql/`, verbatim — one dependency, one decorator.
#[resolver]
pub struct GreetingResolver;

#[operations]
impl GreetingResolver {
    #[query]
    #[public]
    async fn greeting(&self, name: Option<String>) -> String {
        format!("Hello, {}!", name.as_deref().unwrap_or("World"))
    }

    /// A subscription needs a stream type in its signature, and that is the one
    /// place the developer *does* name something — reached here through the
    /// umbrella, which is what proves no second manifest line is forced.
    #[subscription]
    #[public]
    async fn greetings(&self) -> impl Stream<Item = String> {
        stream::iter(["Hello, World!".to_string()])
    }
}

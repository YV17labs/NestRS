//! `#[resolver]` + `#[query]`/`#[mutation]` — the GraphQL operation surface,
//! witnessed through the umbrella alone.
//!
//! What this proves that nothing else does: **`#[resolver]` wraps
//! async-graphql's own `#[Object]`**, whose expansion roots its paths at
//! whatever `proc-macro-crate` finds in the *call site's* manifest — falling
//! back to a bare `::async_graphql` when it finds nothing. That fallback made
//! the first snippet of `/graphql/` uncompilable for anyone who installed the
//! documented `nest-rs = { features = ["graphql"] }` and nothing else. The
//! `crate = ` override `#[resolver]` now threads through is what pins the
//! expansion to the umbrella's re-export, and this module is the compile that
//! fails if it is ever dropped.

use nest_rs::graphql::resolver;

/// The lead snippet of `/graphql/`, verbatim — one dependency, one decorator.
#[resolver]
pub struct GreetingResolver;

#[resolver]
impl GreetingResolver {
    #[query]
    #[public]
    async fn greeting(&self, name: Option<String>) -> String {
        format!("Hello, {}!", name.as_deref().unwrap_or("World"))
    }
}

//! A guard bound where it does not check is the fail-open this bound closes.
//!
//! `Guard::check` admits by default, so without the `GuardFor<GraphqlView>`
//! bound an HTTP-only guard beside a resolver compiled, read as a protection,
//! and passed every operation.

use nest_rs::core::{Layer, UnitContext, injectable};
use nest_rs::graphql::{operations, resolver};
use nest_rs::guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs::http::HttpView;

#[injectable]
#[derive(Default)]
struct HttpOnlyGuard;

impl Layer for HttpOnlyGuard {}

#[async_trait]
impl Guard for HttpOnlyGuard {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for HttpOnlyGuard {}

#[resolver]
#[use_guards(HttpOnlyGuard)]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[public]
    async fn ping(&self) -> i32 {
        0
    }
}

fn main() {}

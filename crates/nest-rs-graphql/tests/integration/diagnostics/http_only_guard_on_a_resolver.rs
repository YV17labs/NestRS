//! A guard bound where it has no `check_*` is the fail-open this bound closes.
//!
//! `Guard::check_graphql` defaults to `Ok(())`, so before the `GraphqlGuard`
//! bound this compiled, read as a protection, and passed every operation — the
//! shape `#[use_guards(ThrottlerGuard)]` beside a `#[query]` had.

use nest_rs_core::{Layer, injectable};
use nest_rs_graphql::{operations, resolver};
use nest_rs_guards::{Denial, Guard, async_trait};
use poem::Request;

#[injectable]
#[derive(Default)]
struct HttpOnlyGuard;

impl Layer for HttpOnlyGuard {}

#[async_trait]
impl Guard for HttpOnlyGuard {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Ok(())
    }
}

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

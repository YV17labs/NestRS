//! A guard bound where it does not check is refused at the binding.
//!
//! An HTTP-only guard's decision reads the request, which a field does not
//! carry: without the `GuardFor<GraphqlView>` bound it would compile beside a
//! resolver, read as a protection, and run on a view it was not written for.

use nest_rs::core::{Layer, UnitContext, async_trait, injectable};
use nest_rs::graphql::{operations, resolver};
use nest_rs::guards::{Denial, Guard, GuardFor};
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

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
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

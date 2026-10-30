//! A guard declaring that it checks GraphQL fields writes the decision a field
//! runs: `Guard::check` has no default, so a guard declaring
//! `GuardFor<GraphqlView>` and writing only `check_http` — which would read as a
//! protection and pass every field — does not compile.

use nest_rs::core::{Layer, UnitContext, injectable};
use nest_rs::graphql::{GraphqlView, operations, resolver};
use nest_rs::guards::{Denial, Guard, GuardFor, async_trait};

#[injectable]
#[derive(Layer)]
struct RequestOnlyGuard;

#[async_trait]
impl Guard for RequestOnlyGuard {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("refused at the request"))
    }
}

impl GuardFor<GraphqlView> for RequestOnlyGuard {}

#[resolver]
#[use_guards(RequestOnlyGuard)]
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

//! The **operation** scope of the same rule, and the sibling of
//! `http_only_guard_on_a_resolver`.
//!
//! `#[resolver]` and `#[operations]` are two decorators emitting the same
//! `GraphqlGuard` bound at two scopes, and `framework.md` asks for a snapshot
//! per site — HTTP's three emitters being the worked example, "each
//! underlin[ing] the decorator the guard was written under, with a snapshot of
//! its own". The resolver scope had one and the operation scope did not, so
//! either half of `#[operations]`' bound could be deleted and the pair stayed
//! proved by the other decorator's snapshot.

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
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[use_guards(HttpOnlyGuard)]
    #[public]
    async fn ping(&self) -> i32 {
        0
    }
}

fn main() {}

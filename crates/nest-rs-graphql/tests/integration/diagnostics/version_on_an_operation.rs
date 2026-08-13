//! `#[version]` is a *route* attribute — it narrows one HTTP route out of the
//! versions its controller mounts. A GraphQL operation has no address to
//! narrow, so writing it beside an operation must be refused by `#[operations]`
//! itself, in the vocabulary the resolver-level refusal already uses. Left to
//! the compiler it is `cannot find attribute `version` in this scope`, which
//! names neither the edge nor the reason.

use nest_rs_graphql::{operations, resolver};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[version("2")]
    #[public]
    async fn ping(&self) -> async_graphql::Result<String> {
        Ok("pong".into())
    }
}

fn main() {}

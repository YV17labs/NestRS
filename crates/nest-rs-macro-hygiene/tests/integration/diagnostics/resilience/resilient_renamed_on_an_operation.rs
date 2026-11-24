//! `#[operations]` reads `#[resilient]` by its name, since a macro resolves
//! none: imported under another, it is handed to the field `#[operations]`
//! generates with the operation's other attributes, whose `self` holds no
//! policy. One error at the attribute says to import it under its own name,
//! where the field's expansion would report a field, a receiver and an answer
//! it cannot have, one of them naming a generated type. The method written is
//! left undecorated, so rustc then reports each answer its body writes as an
//! attempt, in its own words, at the developer's own line.

use std::sync::Arc;

use nest_rs::graphql::{operations, resolver};
use nest_rs::resilience::resilient as shielded;
use nest_rs::resilience::{ResilienceError, ResiliencePolicy};

#[derive(Debug)]
struct UpstreamDown;

impl std::fmt::Display for UpstreamDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the upstream did not answer")
    }
}

impl std::error::Error for UpstreamDown {}

#[resolver]
struct PreviewResolver {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
}

#[operations]
impl PreviewResolver {
    #[query]
    #[public]
    #[shielded(upstream)]
    async fn preview(&self) -> Result<String, ResilienceError<UpstreamDown>> {
        Err(UpstreamDown)
    }
}

fn main() {}

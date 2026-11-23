//! `#[operations]` reads a field's `Result` by its last path segment, so a
//! decorated field answering its `Result<T, ResilienceError<E>>` through an
//! alias would be read whole as the field's GraphQL type, which no
//! `ResilienceError` is. One error at the return type says to spell it out,
//! where the field's expansion would report bound after bound it cannot meet.

use std::sync::Arc;

use nest_rs::graphql::{operations, resolver};
use nest_rs::resilience::{ResilienceError, ResiliencePolicy, resilient};

#[derive(Debug)]
struct UpstreamDown;

impl std::fmt::Display for UpstreamDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the upstream did not answer")
    }
}

impl std::error::Error for UpstreamDown {}

type Answer<T> = Result<T, ResilienceError<UpstreamDown>>;

#[resolver]
struct PreviewResolver {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
}

#[operations]
impl PreviewResolver {
    #[query]
    #[public]
    #[resilient(upstream)]
    async fn preview(&self) -> Answer<String> {
        Err(UpstreamDown)
    }
}

fn main() {}

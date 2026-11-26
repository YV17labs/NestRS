//! `#[operations]` reads a field's `Result` by its last path segment, so a
//! decorated field answering its `Result<T, ResilienceError<E>>` through an
//! alias answers the whole `Result` as its GraphQL type, which none failing
//! with a `ResilienceError` is. It is known there by its type: one sentence at
//! the return type says to spell it out. The bounds the derive and the wrapper
//! then miss follow it in rustc's words, pinned so a change in the first line
//! is seen.

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

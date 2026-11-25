//! `#[tracing::instrument]` written above `#[resilient]` expands first, and the
//! decorator types the body it wrapped as one attempt, in every shape
//! tracing-attributes 0.1.31 writes. These bodies carry by hand a name under
//! that expansion's own prefix — one it binds, then one it does not, below
//! their own block — standing for a release writing another shape or other
//! names: one error says which order compiles whatever tracing writes, where
//! each answer of the body would be reported against a type the decorator
//! could not give it.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::resilience::{ResilienceError, ResiliencePolicy, resilient};

#[derive(Debug)]
struct UpstreamDown;

impl std::fmt::Display for UpstreamDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the upstream did not answer")
    }
}

impl std::error::Error for UpstreamDown {}

#[injectable]
struct PreviewService {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
}

impl PreviewService {
    #[resilient(upstream)]
    async fn preview(&self, slug: String) -> Result<String, ResilienceError<UpstreamDown>> {
        let __tracing_attr_span = slug.len();
        if __tracing_attr_span == 0 {
            return Err(UpstreamDown);
        }
        Ok(slug)
    }

    #[resilient(upstream, idempotent)]
    async fn preview_again(&self, slug: String) -> Result<String, ResilienceError<UpstreamDown>> {
        let traced = async move {
            let __tracing_instrument_span = slug.len();
            if __tracing_instrument_span == 0 {
                return Err(UpstreamDown);
            }
            Ok(slug)
        };
        traced.await
    }
}

fn main() {}

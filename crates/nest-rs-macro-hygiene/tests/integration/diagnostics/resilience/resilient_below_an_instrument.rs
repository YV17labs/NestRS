//! `#[tracing::instrument]` written above `#[resilient]` expands first and
//! wraps the body in a future of its own, answering the method's type where an
//! attempt answers `Result<T, E>`: each such method is one error, saying to
//! write `#[resilient]` above the instrument, the order that compiles whatever
//! tracing writes. The decorator tells the wrapper by the prefix every name
//! tracing-attributes binds shares, so a body carrying one by hand, below its
//! own block, is told the same.

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
    #[tracing::instrument(skip_all)]
    #[resilient(upstream)]
    async fn preview(&self, slug: String) -> Result<String, ResilienceError<UpstreamDown>> {
        if slug.is_empty() {
            return Err(UpstreamDown);
        }
        Ok(slug)
    }

    #[tracing::instrument(skip_all, err, ret)]
    #[resilient(upstream, idempotent)]
    async fn preview_again(&self, slug: String) -> Result<String, ResilienceError<UpstreamDown>> {
        Ok(slug.clone())
    }

    #[resilient(upstream)]
    async fn preview_by_hand(&self, slug: String) -> Result<String, ResilienceError<UpstreamDown>> {
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

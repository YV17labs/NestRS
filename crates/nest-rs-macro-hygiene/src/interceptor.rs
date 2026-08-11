//! `#[interceptor]` — infra a module import auto-mounts at a fixed band, off
//! the layer pool. Its expansion reaches `nest-rs-interceptors` and poem's
//! request/response types, none of which the developer names.

use nest_rs::core::Layer;
use nest_rs::http::interceptor;
use nest_rs::http::poem::{Request, Response, Result};
use nest_rs::interceptors::{Interceptor, Next, async_trait};

/// Minimal infra interceptor. The band is what distinguishes this decorator
/// from a pooled `#[use_interceptors]` binding, so it is stated here.
#[interceptor(priority = -10)]
pub struct HygieneContext;

impl Layer for HygieneContext {}

#[async_trait]
impl Interceptor for HygieneContext {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        next.run(req).await
    }
}

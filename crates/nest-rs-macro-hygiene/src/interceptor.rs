//! `#[interceptor]` — infra a module import auto-mounts at a fixed band.

use nest_rs::core::Layer;
use nest_rs::http::interceptor;
use nest_rs::http::poem::{Request, Response, Result};
use nest_rs::interceptors::{Interceptor, Next, async_trait};

#[interceptor(priority = -10)]
pub struct HygieneContext;

impl Layer for HygieneContext {}

#[async_trait]
impl Interceptor for HygieneContext {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        next.run(req).await
    }
}

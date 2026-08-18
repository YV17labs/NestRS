use nest_rs::core::{Layer, injectable};
use nest_rs::http::async_trait;
use nest_rs::interceptors::{Interceptor, Next};
use poem::{Request, Response, Result};

#[injectable]
#[derive(Default)]
pub struct PostAuditInterceptor;

impl Layer for PostAuditInterceptor {}

#[async_trait]
impl Interceptor for PostAuditInterceptor {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        let resp = next.run(req).await?;

        tracing::info!(
            target: "features::posts",
            status = resp.status().as_u16(),
            "post request audited",
        );
        Ok(resp)
    }
}

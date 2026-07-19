use nest_rs_core::{Layer, injectable};
use nest_rs_http::async_trait;
use nest_rs_interceptors::{Interceptor, Next};
use poem::{Request, Response, Result};

use crate::Claims;

#[injectable]
#[derive(Default)]
pub struct PostAuditInterceptor;

impl Layer for PostAuditInterceptor {}

#[async_trait]
impl Interceptor for PostAuditInterceptor {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        let method = req.method().to_string();
        let path = req.uri().path().to_string();
        let actor = req
            .extensions()
            .get::<Claims>()
            .and_then(|claims| claims.sub)
            .map(|id| id.to_string());

        let resp = next.run(req).await?;

        tracing::info!(
            target: "features::posts",
            method = %method,
            path = %path,
            status = resp.status().as_u16(),
            actor = actor.as_deref(),
            "post request audited",
        );
        Ok(resp)
    }
}

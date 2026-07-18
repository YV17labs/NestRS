use nest_rs_core::{Layer, injectable};
use nest_rs_http::async_trait;
use nest_rs_interceptors::{Interceptor, Next};
use poem::{Request, Response, Result};

use crate::Claims;

/// Audit trail for the posts HTTP surface.
///
/// Bound with `#[use_interceptors(PostAuditInterceptor)]` on `PostsController`,
/// so it wraps every posts handler *inside* the controller's guard chain: a
/// denied request short-circuits at the guard and never reaches this
/// interceptor (guards run before scoped interceptors), meaning every event it
/// emits corresponds to a request the caller was allowed to make.
///
/// It emits exactly one audit event per handled request — a constant
/// event-name message plus structured fields (method, path, status, and the
/// actor read from the request-scoped [`Claims`] the auth guard seeded, when
/// present). Interceptors, unlike reusable pipes, are legitimately
/// app-defined: this is product-specific observability, not a framework
/// primitive.
#[injectable]
#[derive(Default)]
pub struct PostAuditInterceptor;

impl Layer for PostAuditInterceptor {}

#[async_trait]
impl Interceptor for PostAuditInterceptor {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        let method = req.method().to_string();
        let path = req.uri().path().to_string();
        // `AuthGuard` runs at the controller scope, before this method-inside
        // interceptor, so a present `Claims` names the authenticated actor.
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

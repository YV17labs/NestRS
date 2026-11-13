//! [`ReqwestMiddleware`] — reqwest-middleware's seam over the one traced send
//! every reqwest client the framework opens goes through too.

use reqwest::{Request, Response};
use reqwest_middleware::{Error, Middleware, Next, Result};

/// Carries the unit of work's W3C Trace Context onto every request an app's own
/// reqwest client sends — `traceparent` naming a span of the request's own,
/// `tracestate` verbatim, never `baggage` — and, once telemetry exports,
/// records each as the client operation the HTTP conventions name: a `client`
/// span and a point in `http.client.request.duration`, by its method, its
/// server, its status and the HTTP version it was answered over, never its path
/// or its query, where a webhook's secret sits.
///
/// Added once, where the client is built:
///
/// ```
/// use nest_rs_reqwest::ReqwestMiddleware;
///
/// let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
///     .with(ReqwestMiddleware::new())
///     .build();
/// ```
///
/// A request sent outside every unit of work carries no trace context.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReqwestMiddleware {
    _private: (),
}

impl ReqwestMiddleware {
    /// The middleware.
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

#[async_trait::async_trait]
impl Middleware for ReqwestMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        crate::traced::traced(
            req,
            |req| next.run(req, extensions),
            |error| matches!(error, Error::Reqwest(error) if error.is_timeout()),
        )
        .await
    }
}

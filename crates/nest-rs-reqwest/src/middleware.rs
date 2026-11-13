//! [`ReqwestMiddleware`] — reqwest-middleware's seam over the kernel's
//! [`HttpClientRequest`], which every HTTP client the framework opens goes
//! through too.

use nest_rs_core::client_operation::HttpClientRequest;
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
        mut req: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        let url = req.url();
        let request = HttpClientRequest::start(
            req.method(),
            url.scheme(),
            url.host_str(),
            url.port_or_known_default(),
        );
        request.announce(req.headers_mut());
        let answer = next.run(req, extensions).await;
        match &answer {
            Ok(response) => request.answered(response.status(), response.version()),
            Err(Error::Reqwest(error)) if error.is_timeout() => request.timed_out(),
            Err(_) => request.failed(),
        }
        answer
    }
}

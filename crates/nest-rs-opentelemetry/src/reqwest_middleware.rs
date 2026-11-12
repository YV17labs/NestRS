//! [`ReqwestMiddleware`] — what an app adds to its own reqwest client, so every
//! request it sends carries the unit's W3C Trace Context and is recorded as the
//! client operation the HTTP conventions name.

use std::time::{Instant, SystemTime};

use ::reqwest_middleware::{Error, Middleware, Next, Result};
use http::header::HeaderName;
use http::{Extensions, HeaderValue};
use nest_rs_core::Correlation;
use nest_rs_core::client_operation::{self, ClientOperation, ClientValue};
use nest_rs_core::duration_histogram::{DurationHistogram, MetricAttribute, REQUEST_BOUNDARIES};
use nest_rs_core::operation_log::{OTHER, known_verb};
use opentelemetry_semantic_conventions::attribute::{
    HTTP_REQUEST_METHOD, HTTP_RESPONSE_STATUS_CODE, SERVER_ADDRESS, SERVER_PORT, URL_SCHEME,
};
use opentelemetry_semantic_conventions::metric::HTTP_CLIENT_REQUEST_DURATION;
use reqwest::{Request, Response};

/// The HTTP conventions' client duration, keyed by the method, the server and
/// the status it answered.
const REQUEST: DurationHistogram = DurationHistogram::new(
    HTTP_CLIENT_REQUEST_DURATION,
    "Duration of HTTP client requests.",
    REQUEST_BOUNDARIES,
    &[
        MetricAttribute::bounded(HTTP_REQUEST_METHOD),
        MetricAttribute::bounded(SERVER_ADDRESS),
        MetricAttribute::bounded(SERVER_PORT),
        MetricAttribute::bounded(HTTP_RESPONSE_STATUS_CODE),
    ],
);

/// The W3C header naming the trace and the span that sent the request.
const TRACEPARENT: HeaderName = HeaderName::from_static("traceparent");

/// The W3C header carrying the trace's vendor state, forwarded verbatim.
const TRACESTATE: HeaderName = HeaderName::from_static("tracestate");

/// The span name the conventions give a request whose method they do not know.
const UNKNOWN_METHOD_SPAN: &str = "HTTP";

/// The class of a request that never answered because its deadline passed.
const TIMEOUT: &str = "timeout";

/// Carries the unit of work's W3C Trace Context onto every request an app's own
/// reqwest client sends — `traceparent` naming a span of the request's own,
/// `tracestate` verbatim, never `baggage` — and, once telemetry exports,
/// records each as the client operation the HTTP conventions name: a `client`
/// span and a point in `http.client.request.duration`, by its method, its
/// server and the status it answered, never its path or its query, where a
/// webhook's secret sits.
///
/// Added once, where the client is built:
///
/// ```
/// use nest_rs_opentelemetry::ReqwestMiddleware;
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
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        let announced = nest_rs_core::__private::current_correlation().map(|unit| unit.child());
        if let Some(announced) = &announced {
            announce(&mut req, announced);
        }
        if !client_operation::is_recorded() {
            return next.run(req, extensions).await;
        }
        let sent = Sent::of(&req);
        let (started, clock) = (SystemTime::now(), Instant::now());
        let answer = next.run(req, extensions).await;
        sent.record(started, clock, &answer, announced.as_ref());
        answer
    }
}

/// Tell the peer the trace `announced` belongs to and the span it is.
fn announce(req: &mut Request, announced: &Correlation) {
    let headers = req.headers_mut();
    // Hex digits and dashes: always a header value.
    if let Ok(parent) = HeaderValue::from_str(&announced.traceparent().to_string()) {
        headers.insert(TRACEPARENT, parent);
    }
    match announced.tracestate().as_str().map(HeaderValue::from_str) {
        Some(Ok(state)) => {
            headers.insert(TRACESTATE, state);
        }
        // A `tracestate` was read off a header, so it is one; a peer is told
        // nothing rather than half of it.
        Some(Err(_)) | None => {
            headers.remove(TRACESTATE);
        }
    }
}

/// What a request was, read before it is sent: its method as the conventions
/// know it, its scheme and its server.
struct Sent {
    method: &'static str,
    scheme: &'static str,
    address: Option<String>,
    port: Option<u16>,
}

impl Sent {
    fn of(req: &Request) -> Self {
        let method = req.method().as_str();
        let url = req.url();
        Self {
            method: known_verb(method).unwrap_or(OTHER),
            scheme: if url.scheme() == "https" {
                "https"
            } else {
                "http"
            },
            address: url.host_str().map(str::to_owned),
            port: url.port_or_known_default(),
        }
    }

    /// Record the request, sent at `started` (`clock` on the monotonic clock),
    /// as it answered.
    fn record(
        &self,
        started: SystemTime,
        clock: Instant,
        answer: &Result<Response>,
        announced: Option<&Correlation>,
    ) {
        let elapsed = clock.elapsed();
        let status = answer.as_ref().ok().map(Response::status);
        let mut attributes = Vec::with_capacity(6);
        attributes.push((HTTP_REQUEST_METHOD, ClientValue::Text(self.method)));
        attributes.push((URL_SCHEME, ClientValue::Text(self.scheme)));
        if let Some(address) = &self.address {
            attributes.push((SERVER_ADDRESS, ClientValue::Text(address)));
        }
        if let Some(port) = self.port {
            attributes.push((SERVER_PORT, ClientValue::Integer(i64::from(port))));
        }
        if let Some(status) = status {
            attributes.push((
                HTTP_RESPONSE_STATUS_CODE,
                ClientValue::Integer(i64::from(status.as_u16())),
            ));
        }
        let name = if self.method == OTHER {
            UNKNOWN_METHOD_SPAN
        } else {
            self.method
        };
        let failure = match (answer, status.as_ref()) {
            (Ok(_), Some(status)) if status.is_client_error() || status.is_server_error() => {
                Some(status.as_str())
            }
            (Ok(_), _) => None,
            (Err(Error::Reqwest(error)), _) if error.is_timeout() => Some(TIMEOUT),
            (Err(_), _) => Some(OTHER),
        };
        ClientOperation::new(&REQUEST, name, started, elapsed, &attributes)
            .announced_as(announced)
            .failed(failure)
            .record();
    }
}

//! [`ReqwestMiddleware`] — what an app adds to its own reqwest client, so every
//! request it sends carries the unit's W3C Trace Context and is recorded as the
//! client operation the HTTP conventions name.

use std::time::{Instant, SystemTime};

use ::reqwest_middleware::{Error, Middleware, Next, Result};
use http::header::HeaderName;
use http::{Extensions, HeaderValue};
use nest_rs_core::Correlation;
use nest_rs_core::client_operation::{self, ClientDuration, ClientOperation, ClientValue};
use nest_rs_core::operation_log::OTHER;
use reqwest::{Request, Response};

/// The HTTP conventions' client duration, keyed by the method, the server and
/// the status it answered.
const REQUEST: ClientDuration = ClientDuration::new(
    "http.client.request.duration",
    "Duration of HTTP client requests.",
    &[
        HTTP_REQUEST_METHOD,
        SERVER_ADDRESS,
        SERVER_PORT,
        HTTP_RESPONSE_STATUS_CODE,
    ],
);

const HTTP_REQUEST_METHOD: &str = "http.request.method";
const HTTP_RESPONSE_STATUS_CODE: &str = "http.response.status_code";
const SERVER_ADDRESS: &str = "server.address";
const SERVER_PORT: &str = "server.port";
const URL_SCHEME: &str = "url.scheme";

/// The W3C header naming the trace and the span that sent the request.
const TRACEPARENT: HeaderName = HeaderName::from_static("traceparent");

/// The W3C header carrying the trace's vendor state, forwarded verbatim.
const TRACESTATE: HeaderName = HeaderName::from_static("tracestate");

/// The methods the HTTP conventions know; any other reads `_OTHER`.
const KNOWN_METHODS: [&str; 9] = [
    "CONNECT", "DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT", "TRACE",
];

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
            method: KNOWN_METHODS
                .into_iter()
                .find(|known| *known == method)
                .unwrap_or(OTHER),
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
        let mut operation = ClientOperation::new(&REQUEST, name, started, elapsed, &attributes);
        if let Some(announced) = announced {
            operation = operation.announced_as(announced);
        }
        let failure = match (answer, status.as_ref()) {
            (Ok(_), Some(status)) if status.is_client_error() || status.is_server_error() => {
                Some(status.as_str())
            }
            (Ok(_), _) => None,
            (Err(Error::Reqwest(error)), _) if error.is_timeout() => Some(TIMEOUT),
            (Err(_), _) => Some(OTHER),
        };
        match failure {
            Some(failure) => operation.failed(failure).record(),
            None => operation.record(),
        }
    }
}

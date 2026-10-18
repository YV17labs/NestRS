//! W3C Trace Context on HTTP: which headers are read, **whether an inbound one
//! may be believed**, what is echoed back, and the operation span the transport
//! edge opens per request.
//!
//! An inbound `traceparent` is continued only from a peer in
//! `<PREFIX>_HTTP__TRUSTED_PROXIES`, the evidence [`ClientOrigin`] weighs
//! `X-Forwarded-For` on, and restarted otherwise — the specification's front-gate
//! mutation, since an ungated one lets any client pick its trace and set
//! `sampled` on traffic it generates. `X-Request-Id` is upstream data, read
//! behind the same peer and recorded as a captured header, never an identity
//! (`.claude/decisions/trace-context-over-request-id.md`).

use std::fmt::Write;

use http::{HeaderName, HeaderValue, Method, StatusCode, header};
use nest_rs_core::operation_log::OTHER;
use nest_rs_core::{Correlation, TraceParent, TraceState};

use crate::request::Request;
use crate::response::Response;

use crate::client_ip::ClientOrigin;

/// The W3C request header carrying the trace and the caller's span.
pub const TRACEPARENT_HEADER: HeaderName = HeaderName::from_static("traceparent");

/// The W3C request header carrying vendor state, forwarded verbatim.
pub const TRACESTATE_HEADER: HeaderName = HeaderName::from_static("tracestate");

/// The response header reporting the trace this service actually used.
///
/// Standards-track in the W3C working group, not yet a Recommendation.
pub const TRACERESPONSE_HEADER: HeaderName = HeaderName::from_static("traceresponse");

/// The upstream convention, read behind a trusted peer and recorded as an
/// attribute, never an identity.
pub const UPSTREAM_REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// Spelled as OpenTelemetry's registry spells it, so a backend needs no mapping.
const UPSTREAM_REQUEST_ID_ATTR: &str = "http.request.header.x-request-id";

/// The trace this request belongs to: the caller's when a trusted proxy
/// forwarded a valid `traceparent`, a freshly started one otherwise.
pub(crate) fn resolve(req: &Request, origin: ClientOrigin) -> Correlation {
    continued(req, origin).unwrap_or_else(|| Correlation::minted(None))
}

/// The inbound context, if there is one and it may be believed.
fn continued(req: &Request, origin: ClientOrigin) -> Option<Correlation> {
    if !origin.peer_is_trusted() {
        return None;
    }
    let parent = TraceParent::parse(claimed_traceparent(req)?)?;
    let tracestate = req
        .headers()
        .get(TRACESTATE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(TraceState::adopt)
        .unwrap_or_default();
    Some(Correlation::continued(parent, tracestate, None))
}

/// What the caller claims, believed or not.
fn claimed_traceparent(req: &Request) -> Option<&str> {
    req.headers()
        .get(TRACEPARENT_HEADER)?
        .to_str()
        .ok()
        .filter(|raw| !raw.is_empty())
}

/// What the caller claimed, as far as it parsed: a W3C `traceparent` is
/// bounded hex up to its 55th character, and a later version's further fields
/// are none of it.
fn parsed_traceparent(claimed: &str) -> Option<&str> {
    TraceParent::parse(claimed)?;
    claimed.trim().get(..TRACEPARENT_LEN)
}

/// The upstream proxy's own request id, when a trusted peer sent one in the
/// shape a proxy mints: no specification gives `X-Request-Id` a grammar, and a
/// client may write the header through a proxy that forwards it, so only
/// bounded hex is recorded — a UUID (RFC 9562 §4), or 16 to 64 hex digits as
/// nginx's `$request_id` is 32 — never a word a client could have chosen.
fn upstream_request_id(req: &Request, origin: ClientOrigin) -> Option<&str> {
    origin.peer_is_trusted().then_some(())?;
    req.headers()
        .get(UPSTREAM_REQUEST_ID_HEADER)?
        .to_str()
        .ok()
        .filter(|raw| is_minted_id(raw))
}

/// Whether `id` is a UUID's `8-4-4-4-12` hex groups, or 16 to 64 hex digits.
fn is_minted_id(id: &str) -> bool {
    const UUID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let hex = |group: &str| group.bytes().all(|byte| byte.is_ascii_hexdigit());
    if id.contains('-') {
        let mut groups = id.split('-');
        return UUID_GROUPS.iter().all(|len| {
            groups
                .next()
                .is_some_and(|group| group.len() == *len && hex(group))
        }) && groups.next().is_none();
    }
    (16..=64).contains(&id.len()) && hex(id)
}

/// `method` as the HTTP conventions name it: one they know — RFC 9110's, PATCH
/// (RFC 5789) and QUERY — spelled exactly, methods being case-sensitive;
/// [`OTHER`] for any other, a token the client chose.
///
/// No override list: the router serves no method outside this set, so a
/// method an app answers is never `_OTHER`.
pub(crate) fn method_name(method: &Method) -> &'static str {
    match method.as_str() {
        "CONNECT" => "CONNECT",
        "DELETE" => "DELETE",
        "GET" => "GET",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        "PATCH" => "PATCH",
        "POST" => "POST",
        "PUT" => "PUT",
        "QUERY" => "QUERY",
        "TRACE" => "TRACE",
        _ => OTHER,
    }
}

/// Report the trace this service used back to the caller.
pub(crate) fn stamp(correlation: &Correlation, resp: &mut Response) {
    // A stack buffer: `to_string()` would allocate twice per response.
    let mut buf = [0_u8; TRACEPARENT_LEN];
    let mut cursor = Cursor {
        buf: &mut buf,
        at: 0,
    };
    if write!(cursor, "{}", correlation.traceparent()).is_err() {
        return;
    }
    if let Ok(value) = HeaderValue::from_bytes(&buf) {
        resp.headers_mut().insert(TRACERESPONSE_HEADER, value);
    }
}

/// `00-` + 32 + `-` + 16 + `-` + 2, for the one version this framework writes.
const TRACEPARENT_LEN: usize = 55;

/// A `fmt::Write` over a fixed buffer that refuses rather than truncates: a
/// half-written trace context is worse for a caller than none.
struct Cursor<'a> {
    buf: &'a mut [u8],
    at: usize,
}

impl std::fmt::Write for Cursor<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.at + s.len();
        let room = self.buf.get_mut(self.at..end).ok_or(std::fmt::Error)?;
        room.copy_from_slice(s.as_bytes());
        self.at = end;
        Ok(())
    }
}

/// Open the **operation span** for one HTTP request — the span every event below
/// it inherits, and the one an OTel exporter turns into a server span.
///
/// Opened here, not in `nest-rs-opentelemetry`: `tracing` fixes a span's fields
/// at creation, and `AuthnGuard`'s `actor_id` must land whether or not that
/// crate is installed.
pub(crate) fn request_span(
    req: &Request,
    correlation: &Correlation,
    origin: ClientOrigin,
) -> tracing::Span {
    let method = method_name(req.method());
    let span = nest_rs_core::operation_span!(
        crate::unit::REQUEST,
        correlation,
        // `tracing` fixes a span name to a literal; `tracing-opentelemetry`
        // reads `otel.name` as the override, filled by `name_route`.
        otel.name = tracing::field::Empty,
        http.request.method = method,
        // The conventions require it wherever it differs from the method named.
        http.request.method_original = (method == OTHER).then(|| req.method().as_str()),
        // As addressed; never in `http.route`, which a backend groups on.
        url.path = req.uri().path(),
        // Filled by `name_route`; a 404 matched no route and leaves it empty.
        http.route = tracing::field::Empty,
        // No `client.address`: an address is personal data a collector would keep.
        user_agent.original = req
            .headers()
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default(),
        // What the caller claimed, believed or not, once it parsed.
        "http.request.header.traceparent" = tracing::field::Empty,
        "http.request.header.x-request-id" = tracing::field::Empty,
        // The size only when the access log is on, which pays for counting.
        http.response.status_code = tracing::field::Empty,
        http.response.body.size = tracing::field::Empty,
    );
    // On a continued trace, `trace_id` already says it.
    if correlation.parent_id().is_none()
        && let Some(claimed) = claimed_traceparent(req).and_then(parsed_traceparent)
    {
        span.record("http.request.header.traceparent", claimed);
    }
    if let Some(upstream) = upstream_request_id(req, origin) {
        span.record(UPSTREAM_REQUEST_ID_ATTR, upstream);
    }
    span
}

/// Name the span for what the router matched: `http.route` is the template,
/// `otel.name` is `{method} {route}`, or the method alone when nothing matched;
/// a method the conventions do not know is `HTTP` there, as they spell it.
///
/// Never the raw path: one scanner would fill a tracing backend with span names.
pub(crate) fn name_route(span: &tracing::Span, method: &Method, route: Option<&str>) {
    let method = match method_name(method) {
        OTHER => "HTTP",
        known => known,
    };
    match route {
        Some(route) => {
            span.record("http.route", route);
            span.record(
                "otel.name",
                tracing::field::display(format_args!("{method} {route}")),
            );
        }
        None => {
            span.record("otel.name", tracing::field::display(method));
        }
    }
}

/// Record on the span that the request failed: a `5xx` only, which
/// OpenTelemetry's HTTP conventions read as a failed server operation.
pub(crate) fn record_failure(span: &tracing::Span, status: StatusCode) {
    if status.is_server_error() {
        nest_rs_core::operation_log::record_error(span, status.as_str());
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::*;

    /// The specification's own example header.
    const UPSTREAM: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    const UPSTREAM_TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test literal is an IP")
    }

    fn resolve(req: &Request, trusted_proxies: &[IpAddr]) -> Correlation {
        super::resolve(req, ClientOrigin::of_with(req, trusted_proxies))
    }

    /// A request from `peer`, carrying `headers`.
    fn req_from(peer: &str, headers: &[(HeaderName, &str)]) -> Request {
        let socket: std::net::SocketAddr = format!("{peer}:40000").parse().expect("test literal");
        let mut built = Request::builder().peer_addr(socket);
        for (name, value) in headers {
            built = built.header(
                name.clone(),
                HeaderValue::from_str(value).expect("header value"),
            );
        }
        built.finish()
    }

    #[test]
    fn an_inbound_traceparent_is_ignored_with_no_trusted_proxy() {
        let req = req_from("10.0.0.1", &[(TRACEPARENT_HEADER, UPSTREAM)]);
        let correlation = resolve(&req, &[]);
        assert_ne!(
            correlation.trace_id().to_hex(),
            UPSTREAM_TRACE,
            "an ungated header lets one caller file into another's trace",
        );
        assert_eq!(
            correlation.parent_id(),
            None,
            "a restarted trace has no parent — it is the root",
        );
    }

    #[test]
    fn a_trusted_traceparent_is_continued_with_the_caller_as_parent() {
        let req = req_from("10.0.0.1", &[(TRACEPARENT_HEADER, UPSTREAM)]);
        let correlation = resolve(&req, &[ip("10.0.0.1")]);

        assert_eq!(correlation.trace_id().to_hex(), UPSTREAM_TRACE);
        assert_eq!(
            correlation.parent_id().map(|id| id.to_hex()),
            Some(String::from("00f067aa0ba902b7")),
        );
        assert_ne!(
            correlation.span_id().to_hex(),
            "00f067aa0ba902b7",
            "this request is its own unit of work, not the caller's",
        );
    }

    #[test]
    fn an_inbound_traceparent_from_an_untrusted_peer_is_ignored() {
        let req = req_from("203.0.113.9", &[(TRACEPARENT_HEADER, UPSTREAM)]);
        assert_ne!(
            resolve(&req, &[ip("10.0.0.1")]).trace_id().to_hex(),
            UPSTREAM_TRACE,
        );
    }

    #[test]
    fn a_malformed_traceparent_restarts_rather_than_failing() {
        let req = req_from("10.0.0.1", &[(TRACEPARENT_HEADER, "not-a-traceparent")]);
        let correlation = resolve(&req, &[ip("10.0.0.1")]);
        assert!(correlation.parent_id().is_none());
        assert!(
            correlation.flags().is_sampled(),
            "a fresh trace is ours to decide"
        );
    }

    #[test]
    fn tracestate_is_dropped_when_the_traceparent_is_not_continued() {
        let req = req_from(
            "10.0.0.1",
            &[
                (TRACEPARENT_HEADER, "garbage"),
                (TRACESTATE_HEADER, "rojo=00f067aa0ba902b7"),
            ],
        );
        assert_eq!(resolve(&req, &[ip("10.0.0.1")]).tracestate().as_str(), None);
    }

    #[test]
    fn tracestate_is_carried_when_the_trace_is_continued() {
        let req = req_from(
            "10.0.0.1",
            &[
                (TRACEPARENT_HEADER, UPSTREAM),
                (TRACESTATE_HEADER, "rojo=00f067aa0ba902b7,congo=t61rcWkgMzE"),
            ],
        );
        assert_eq!(
            resolve(&req, &[ip("10.0.0.1")]).tracestate().as_str(),
            Some("rojo=00f067aa0ba902b7,congo=t61rcWkgMzE"),
        );
    }

    #[test]
    fn a_request_with_no_trace_headers_still_starts_one() {
        let req = req_from("10.0.0.1", &[]);
        let correlation = resolve(&req, &[ip("10.0.0.1")]);
        assert_eq!(correlation.trace_id().to_hex().len(), 32);
        assert!(correlation.parent_id().is_none());
    }

    /// nginx's `$request_id`: 32 hex digits.
    const NGINX_REQUEST_ID: &str = "8f14e45fceea167a5a36dedd4bea2543";

    #[test]
    fn an_upstream_request_id_is_read_only_behind_a_trusted_peer() {
        let headers = [(UPSTREAM_REQUEST_ID_HEADER, NGINX_REQUEST_ID)];
        let trusted = req_from("10.0.0.1", &headers);
        let untrusted = req_from("203.0.113.9", &headers);

        assert_eq!(
            upstream_request_id(&trusted, ClientOrigin::of_with(&trusted, &[ip("10.0.0.1")])),
            Some(NGINX_REQUEST_ID),
        );
        assert_eq!(
            upstream_request_id(
                &untrusted,
                ClientOrigin::of_with(&untrusted, &[ip("10.0.0.1")])
            ),
            None,
        );
    }

    /// No specification gives `X-Request-Id` a grammar: it is recorded in the
    /// shape a proxy mints, bounded hex, and not at all when it is a word a
    /// client could have written through the proxy, a secret's shape included.
    #[test]
    fn an_upstream_request_id_is_recorded_only_as_a_minted_id() {
        let trusted = [ip("10.0.0.1")];
        let at_most = "f".repeat(64);
        let beyond = "f".repeat(65);
        for (sent, recorded) in [
            (NGINX_REQUEST_ID, true),
            ("0190e0b4-7d3c-7c4e-9f1a-3b2c1d0e9f8a", true),
            ("0190E0B4-7D3C-7C4E-9F1A-3B2C1D0E9F8A", true),
            ("00f067aa0ba902b7", true),
            (at_most.as_str(), true),
            (beyond.as_str(), false),
            ("00f067aa0ba902b", false),
            ("sk_live_51HsecretTOKEN", false),
            ("nginx-abc123", false),
            ("free text marked", false),
            ("0190e0b4-7d3c-7c4e-9f1a-3b2c1d0e9f8a-marked", false),
            ("0190e0b4-7d3c-7c4e-9f1a3b2c1d0e9f8a", false),
            ("0190e0b4-7d3c-7c4e-9f1a-3b2c1d0e9f8z", false),
            ("\"quoted\"", false),
        ] {
            let req = req_from("10.0.0.1", &[(UPSTREAM_REQUEST_ID_HEADER, sent)]);
            assert_eq!(
                upstream_request_id(&req, ClientOrigin::of_with(&req, &trusted)),
                recorded.then_some(sent),
                "{sent}",
            );
        }
    }

    /// The conventions' known methods, spelled exactly; any other is `_OTHER`.
    #[test]
    fn a_method_is_named_from_the_known_set_or_other() {
        for known in [
            "CONNECT", "DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT", "QUERY", "TRACE",
        ] {
            let method = Method::from_bytes(known.as_bytes()).expect("a method token");
            assert_eq!(method_name(&method), known);
        }
        for other in ["get", "Get", "PROPFIND", "MARKED"] {
            let method = Method::from_bytes(other.as_bytes()).expect("a method token");
            assert_eq!(method_name(&method), OTHER, "{other}");
        }
    }

    #[test]
    fn the_response_reports_the_trace_this_service_used() {
        let correlation = Correlation::minted(None);
        let mut resp = Response::default();
        stamp(&correlation, &mut resp);

        let echoed = resp
            .headers()
            .get(TRACERESPONSE_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("the header is stamped");
        let parsed = TraceParent::parse(echoed).expect("and it is a valid traceparent");
        assert_eq!(parsed.trace_id, correlation.trace_id());
        assert_eq!(
            parsed.parent_id,
            correlation.span_id(),
            "the span we served it under, so a caller can name it exactly",
        );
    }
}

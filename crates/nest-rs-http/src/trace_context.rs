//! W3C Trace Context on HTTP: which headers are read, **whether an inbound one
//! may be believed**, what is echoed back, and the operation span the transport
//! edge opens per request.
//!
//! The context itself is the kernel's primitive
//! ([`nest_rs_core::trace_context`]) — this module owns only what is genuinely
//! HTTP's. None of it is the access log's, which is why none of it lives there:
//! turning the log off does not turn off the trace.
//!
//! # The trust decision, and why it is the same one as the client address
//!
//! `traceparent` is a caller-authored string. Honouring it unconditionally lets
//! any client file its request into a trace of its choosing — splicing itself
//! into somebody else's, or, with the `sampled` flag set, doing so at whatever
//! volume it likes. That second one is the attack the specification names:
//!
//! > *When distributed tracing is enabled on a service with a public API and
//! > naively continues any trace with the `sampled` flag set, a malicious
//! > attacker could overwhelm an application.*
//!
//! Ignoring it entirely would break the case that matters — an infrastructure
//! hop or a sibling service that is already tracing. So it is read on exactly
//! the evidence [`ClientOrigin`](crate::ClientOrigin) uses for
//! `X-Forwarded-For`: the direct peer is in `NESTRS_HTTP__TRUSTED_PROXIES`. One
//! list, one decision, and a deployment cannot end up believing a header for the
//! client's address while disbelieving it for the trace, or the reverse.
//!
//! With no proxy declared — the default — every request **restarts the trace**,
//! which is the mutation the specification defines for exactly this position:
//!
//! > *Restart trace: All properties (`trace-id`, `parent-id`, `trace-flags`) are
//! > regenerated. This mutation is used in services that are defined as a front
//! > gate into secure networks and eliminates a potential denial-of-service
//! > attack surface.*
//!
//! Nothing is lost by restarting: the caller's claimed `traceparent` is recorded
//! on the span, so a deployment that wants to join against it still can.
//!
//! # `X-Request-Id` is upstream data, not our identity
//!
//! It is a de-facto convention (nginx, Envoy, Heroku), never a standard, and
//! OpenTelemetry's semantic conventions place it exactly where it belongs: as
//! `http.request.header.x-request-id`, a *captured header*. So it is read behind
//! the same trusted peer and recorded as an attribute — enough to join our
//! events to the proxy's log line — and it never decides what this request is
//! called. That is `trace_id`'s job, and there is only one of it.

use std::fmt::Write;

use nest_rs_core::{Correlation, TraceParent, TraceState};
use poem::http::{HeaderName, HeaderValue, Method, header};
use poem::{Request, Response};

use crate::client_ip::{ClientIp, ClientOrigin};

/// The W3C request header carrying the trace and the caller's span.
pub const TRACEPARENT_HEADER: HeaderName = HeaderName::from_static("traceparent");

/// The W3C request header carrying vendor state, forwarded verbatim.
pub const TRACESTATE_HEADER: HeaderName = HeaderName::from_static("tracestate");

/// The response header reporting the trace this service actually used.
///
/// **Standards-track, not yet a Recommendation** — and that is stated rather
/// than hidden. `traceparent` and `tracestate` are a W3C Recommendation; the
/// response side is still being specified, with `traceresponse` as the working
/// group's form and the same `version-traceid-spanid-flags` shape.
///
/// It is emitted anyway, because the alternative is a house header and because
/// its stated purpose is precisely this framework's position: a service called
/// by an untrusted third party, which **restarted the trace**, telling the
/// caller which trace it really used. A caller quoting an id in a bug report
/// reads it here.
pub const TRACERESPONSE_HEADER: HeaderName = HeaderName::from_static("traceresponse");

/// The upstream convention, read behind a trusted peer and recorded as an
/// attribute. Never an identity — see the module docs.
pub const UPSTREAM_REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// The semantic-convention attribute an upstream `X-Request-Id` is recorded
/// under, spelled exactly as OpenTelemetry's registry spells it so a backend
/// needs no per-deployment field mapping.
const UPSTREAM_REQUEST_ID_ATTR: &str = "http.request.header.x-request-id";

/// The trace this request belongs to: the caller's when a trusted proxy
/// forwarded a valid `traceparent`, a freshly started one otherwise.
///
/// `origin` is the *already resolved* client origin rather than the proxy list,
/// so the trust decision is made once per request and this module cannot come to
/// disagree with [`ClientOrigin`] about which peers are infrastructure.
pub(crate) fn resolve(req: &Request, origin: ClientOrigin) -> Correlation {
    continued(req, origin).unwrap_or_else(Correlation::mint)
}

/// The inbound context, if there is one and it may be believed.
///
/// `tracestate` is only ever adopted **beside a valid `traceparent`** — the
/// specification is explicit that state without the parent it belongs to is
/// state for a trace we are not continuing.
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

/// What the caller claims, believed or not. Read for the span attribute as well
/// as for the trust decision, so a restarted trace still says what it refused.
fn claimed_traceparent(req: &Request) -> Option<&str> {
    req.headers()
        .get(TRACEPARENT_HEADER)?
        .to_str()
        .ok()
        .filter(|raw| !raw.is_empty())
}

/// The upstream proxy's own request id, when a trusted peer sent one.
fn upstream_request_id(req: &Request, origin: ClientOrigin) -> Option<&str> {
    origin.peer_is_trusted().then_some(())?;
    req.headers()
        .get(UPSTREAM_REQUEST_ID_HEADER)?
        .to_str()
        .ok()
        .filter(|raw| !raw.is_empty() && raw.len() <= MAX_UPSTREAM_LEN)
}

/// Longest upstream id recorded. It is echoed into structured logs, so one
/// caller does not get to make every line unreadable.
const MAX_UPSTREAM_LEN: usize = 200;

/// Report the trace this service used back to the caller.
///
/// Stamped whether or not the access line is emitted: a caller quoting this in a
/// bug report is the point, and an id that comes back only on some deployments
/// is one nobody learns to quote. A value that cannot become a header is dropped
/// rather than panicking — the format is hex and dashes, so this is the belt to
/// that brace.
pub(crate) fn stamp(correlation: &Correlation, resp: &mut Response) {
    // Formatted into a stack buffer rather than through `to_string()`: the
    // length is fixed by the grammar, and going through a `String` would
    // allocate once for it and once more inside `HeaderValue`, per response.
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

/// `00-` + 32 + `-` + 16 + `-` + 2 — fixed by the grammar for the one version
/// this framework writes.
const TRACEPARENT_LEN: usize = 55;

/// A `fmt::Write` over a fixed buffer, so the header is built without touching
/// the allocator. Refuses rather than truncates: a half-written trace context is
/// worse for a caller than none.
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
/// It is opened by the transport, and not by `nest-rs-opentelemetry`, because
/// the field it declares for `actor_id` is what makes `AuthnGuard`'s
/// `Span::current().record("actor_id", …)` land. `tracing` fixes a span's fields
/// at creation, so a span the observability crate owns means an `actor_id` that
/// exists only when that crate is installed — which is exactly how it came to be
/// recorded nowhere at all.
///
/// **The OTel semantic-convention names are deliberate.** They are the industry
/// spelling of a server span, so an export is correct with nothing to translate,
/// and declaring them here is what lets there be *one* span per request instead
/// of the framework's and the exporter's side by side. Without OTel mounted they
/// are inert fields on a console line.
///
/// Values are borrowed straight off the request: `tracing` records field values
/// eagerly at creation, so nothing here allocates.
pub(crate) fn request_span(
    req: &Request,
    correlation: &Correlation,
    client: &ClientIp,
    origin: ClientOrigin,
    user_agent: Option<&str>,
) -> tracing::Span {
    let span = nest_rs_core::operation_span!(
        target: "nest_rs::http",
        kind: "server",
        "http.request",
        correlation,
        // The exported span's name. OpenTelemetry's HTTP conventions want
        // `{method} {route}`, and `tracing` fixes a span name to a literal — so
        // one literal would have to serve every route, and a backend that groups
        // by span name would render this whole deployment as a single line.
        // `otel.name` is the override `tracing-opentelemetry` reads, and it is
        // filled by [`name_route`] once the router has answered.
        otel.name = tracing::field::Empty,
        http.request.method = %req.method(),
        // The path **as addressed**, which is what `url.path` means. The matched
        // template goes to `http.route`, below, and the two are not
        // interchangeable: a conventions-aware backend groups on `http.route`, so
        // putting `/users/01a0…` there is the cardinality explosion the split
        // exists to prevent.
        url.path = req.uri().path(),
        // Filled by [`name_route`]: the router runs *inside* this span, which is
        // also why a 404 is observed at all — and a 404 matched no route, so the
        // field staying empty is the answer rather than a gap.
        http.route = tracing::field::Empty,
        client.address = %client.ip,
        user_agent.original = user_agent.unwrap_or_default(),
        // What the caller *claimed*, recorded whether or not it was believed —
        // so a restarted trace still says which trace it declined to join, and a
        // deployment that wants to correlate across the boundary can.
        "http.request.header.traceparent" = tracing::field::Empty,
        // The upstream proxy's own id, for joining against its access log.
        "http.request.header.x-request-id" = tracing::field::Empty,
        // Recorded on the way out — always for the status, and for the size only
        // when the access log is on, since that is what pays for counting.
        http.response.status_code = tracing::field::Empty,
        http.response.body.size = tracing::field::Empty,
    );
    // Only when it differs from what we adopted: on a continued trace the header
    // is already said by `trace_id`, and *one event, said once* is the rule.
    if correlation.parent_id().is_none()
        && let Some(claimed) = claimed_traceparent(req)
    {
        span.record("http.request.header.traceparent", claimed);
    }
    if let Some(upstream) = upstream_request_id(req, origin) {
        span.record(UPSTREAM_REQUEST_ID_ATTR, upstream);
    }
    span
}

/// Name the span for what the router actually matched.
///
/// Two conventions are settled here and they are one decision, because both need
/// the answer only the router has:
///
/// - **`http.route`** is the low-cardinality *template* (`/users/:id`), never the
///   path as addressed. A backend groups latency and error rates on it, so a raw
///   path there produces one group per identifier and no signal at all.
/// - **`otel.name`** is `{method} {route}` — the industry spelling of a server
///   span. Without it every request in the deployment exports under one name and
///   a trace list is unreadable.
///
/// With no match — a 404, a path no controller claimed — the name is the method
/// alone. That is the conventions' own fallback, and it is deliberately *not*
/// the raw path: naming an unmatched span after the URL is how one scanner
/// filling a backend with junk span names becomes an incident of its own.
pub(crate) fn name_route(span: &tracing::Span, method: &Method, route: Option<&str>) {
    match route {
        Some(route) => {
            span.record("http.route", route);
            span.record(
                "otel.name",
                tracing::field::display(format_args!("{method} {route}")),
            );
        }
        // The conventions' own fallback, and deliberately *not* the raw path.
        None => {
            span.record("otel.name", tracing::field::display(method));
        }
    }
}

/// The user agent, read once per request: the operation span and the access line
/// both want it, and a header parsed twice is two chances to disagree.
pub(crate) fn user_agent(req: &Request) -> Option<&str> {
    req.headers()
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::*;

    /// The spec's own example header, used everywhere below so a failure is
    /// never about a hand-typed hex string.
    const UPSTREAM: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    const UPSTREAM_TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test literal is an IP")
    }

    /// The edge's own sequence: resolve the origin against the declared list,
    /// then decide the trace on it. Asserting through it is what keeps the two
    /// halves of the trust decision tested together.
    fn resolve(req: &Request, trusted_proxies: &[IpAddr]) -> Correlation {
        super::resolve(req, ClientOrigin::of_with(req, trusted_proxies))
    }

    /// A request from `peer`, carrying whatever headers the case needs. Built
    /// through `from_parts` because that is the only way to give a `Request` a
    /// remote address, and the peer *is* the evidence under test.
    fn req_from(peer: &str, headers: &[(HeaderName, &str)]) -> Request {
        use poem::Addr;
        use poem::web::{LocalAddr, RemoteAddr};

        let socket: std::net::SocketAddr = format!("{peer}:40000").parse().expect("test literal");
        let (mut parts, _) = poem::http::Request::new(()).into_parts();
        for (name, value) in headers {
            parts
                .headers
                .insert(name, HeaderValue::from_str(value).expect("header value"));
        }
        Request::from_parts(
            (
                parts,
                LocalAddr::default(),
                RemoteAddr(Addr::socket(socket)),
                poem::http::uri::Scheme::HTTP,
            )
                .into(),
            poem::Body::empty(),
        )
    }

    /// The default deployment trusts nobody, so a caller cannot splice itself
    /// into a trace of its choosing — nor set the `sampled` flag on traffic it
    /// generates, which is the denial-of-service surface the spec names.
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

    /// The same header and the same value, continued only because the peer is
    /// declared infrastructure. The caller's span becomes ours' parent, which is
    /// the whole point of continuing rather than restarting.
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

    /// A malformed header from a peer we *do* trust is not a reason to refuse to
    /// serve anyone — it restarts the trace, exactly as an absent one would.
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

    /// State without the parent it belongs to is state for a trace we are not
    /// continuing, so it goes with it.
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

    /// And carried verbatim when it is — a MUST, and one whose breakage is
    /// invisible to us and fatal to whichever vendor's routing rides in it.
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

    /// Absence of a header is never absence of a trace.
    #[test]
    fn a_request_with_no_trace_headers_still_starts_one() {
        let req = req_from("10.0.0.1", &[]);
        let correlation = resolve(&req, &[ip("10.0.0.1")]);
        assert_eq!(correlation.trace_id().to_hex().len(), 32);
        assert!(correlation.parent_id().is_none());
    }

    /// `X-Request-Id` is upstream data behind a trusted peer, and nothing at
    /// all otherwise. Either way it never decides the trace.
    #[test]
    fn an_upstream_request_id_is_read_only_behind_a_trusted_peer() {
        let headers = [(UPSTREAM_REQUEST_ID_HEADER, "nginx-abc123")];
        let trusted = req_from("10.0.0.1", &headers);
        let untrusted = req_from("203.0.113.9", &headers);

        assert_eq!(
            upstream_request_id(&trusted, ClientOrigin::of_with(&trusted, &[ip("10.0.0.1")])),
            Some("nginx-abc123"),
        );
        assert_eq!(
            upstream_request_id(
                &untrusted,
                ClientOrigin::of_with(&untrusted, &[ip("10.0.0.1")])
            ),
            None,
        );
    }

    /// What the caller reads back is the trace we actually used — which, at a
    /// front gate, is deliberately not the one they asked for.
    #[test]
    fn the_response_reports_the_trace_this_service_used() {
        let correlation = Correlation::mint();
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

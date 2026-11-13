//! One operation this process asked of another — a statement sent to a
//! database, a command sent to a store, a request sent to an HTTP server —
//! timed by the client that sent it and handed, once it ended, to whatever
//! records it.
//!
//! The observability stack seeds the recorder at boot; with none seeded,
//! [`is_recorded`] is `false` and a client times nothing. What an operation
//! carries is the server's: its name and its attributes say which operation
//! and on what, never a value it sent. A client times one with a
//! [`PendingClientOperation`], which records it however it ends — dropped
//! included — and an HTTP request with an [`HttpClientRequest`], whichever
//! client sends it.

use std::borrow::Cow;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use http::header::{HeaderMap, HeaderName, HeaderValue};
use http::{Method, StatusCode, Version};

use crate::Correlation;
use crate::duration_histogram::{DurationHistogram, MetricAttribute, REQUEST_BOUNDARIES};
use crate::operation_log::{OTHER, known_verb};

/// The database conventions' client duration, which every database driver
/// records its operations in.
pub const DB_CLIENT_OPERATION_DURATION: DurationHistogram = DurationHistogram::new(
    "db.client.operation.duration",
    "Duration of database client operations.",
    &[0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0],
    &[
        MetricAttribute::bounded(DB_SYSTEM_NAME),
        MetricAttribute::bounded(DB_NAMESPACE),
        MetricAttribute::bounded(DB_OPERATION_NAME),
        MetricAttribute::bounded(DB_COLLECTION_NAME),
        MetricAttribute::bounded(DB_QUERY_SUMMARY),
        MetricAttribute::bounded(SERVER_ADDRESS),
        MetricAttribute::bounded(SERVER_PORT),
        MetricAttribute::bounded(DB_RESPONSE_STATUS_CODE),
    ],
);

/// The code a database answered a failed operation with, which its
/// `error.type` repeats: a Postgres SQLSTATE (`40001`), a Redis error prefix
/// (`WRONGTYPE`).
pub const DB_RESPONSE_STATUS_CODE: &str = "db.response.status_code";

/// The database system, as the conventions name it (`postgresql`, `redis`).
pub const DB_SYSTEM_NAME: &str = "db.system.name";
/// The database the operation ran in.
pub const DB_NAMESPACE: &str = "db.namespace";
/// What the operation did (`SELECT`, `GET`).
pub const DB_OPERATION_NAME: &str = "db.operation.name";
/// The table or collection it did it to.
pub const DB_COLLECTION_NAME: &str = "db.collection.name";
/// The operation and its target together (`SELECT posts`).
pub const DB_QUERY_SUMMARY: &str = "db.query.summary";
/// The server the operation was sent to.
pub const SERVER_ADDRESS: &str = "server.address";
/// The port it was sent to.
pub const SERVER_PORT: &str = "server.port";

/// A value one of an operation's attributes holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientValue<'a> {
    /// A string.
    Text(&'a str),
    /// An integer.
    Integer(i64),
}

/// One client operation, as it ended: what it was, when it started and how
/// long it took, the attributes its conventions name, how it failed, and its
/// place in the trace of the unit that sent it.
#[derive(Clone, Copy, Debug)]
pub struct ClientOperation<'a> {
    duration: &'static DurationHistogram,
    name: &'a str,
    started: SystemTime,
    elapsed: Duration,
    attributes: &'a [(&'static str, ClientValue<'a>)],
    error_type: Option<&'a str>,
    correlation: Option<&'a Correlation>,
}

impl<'a> ClientOperation<'a> {
    /// The operation `name`, recorded in `duration`, started at `started` and
    /// ended `elapsed` later, carrying `attributes`.
    pub const fn new(
        duration: &'static DurationHistogram,
        name: &'a str,
        started: SystemTime,
        elapsed: Duration,
        attributes: &'a [(&'static str, ClientValue<'a>)],
    ) -> Self {
        Self {
            duration,
            name,
            started,
            elapsed,
            attributes,
            error_type: None,
            correlation: None,
        }
    }

    /// The same operation, failed with the class `failure` names — a status
    /// code its protocol answered, or `_OTHER` — if it failed.
    pub const fn failed(self, failure: Option<&'a str>) -> Self {
        Self {
            error_type: failure,
            ..self
        }
    }

    /// The same operation, correlated as `correlation`: a child of the unit
    /// that sent it, whose span id is the operation's span — the one a
    /// `traceparent` sent with it named as its peer's parent.
    pub const fn correlated(self, correlation: Option<&'a Correlation>) -> Self {
        Self {
            correlation,
            ..self
        }
    }

    /// The histogram it records in.
    pub const fn duration(&self) -> &'static DurationHistogram {
        self.duration
    }

    /// Its span's name, as its conventions build it (`SELECT posts`).
    pub const fn name(&self) -> &'a str {
        self.name
    }

    /// When it was sent.
    pub const fn started(&self) -> SystemTime {
        self.started
    }

    /// How long it took to end.
    pub const fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// The attributes its conventions name.
    pub const fn attributes(&self) -> &'a [(&'static str, ClientValue<'a>)] {
        self.attributes
    }

    /// How it failed, if it did.
    pub const fn error_type(&self) -> Option<&'a str> {
        self.error_type
    }

    /// Its place in the trace of the unit that sent it, where its client gave
    /// one; a recorder places one given none under the unit it is recorded in.
    pub const fn correlation(&self) -> Option<&'a Correlation> {
        self.correlation
    }

    /// Hand it to the recorder the observability stack seeded; nothing
    /// happens where none was.
    pub fn record(&self) {
        if let Some(recorder) = RECORDER.get() {
            recorder(self);
        }
    }
}

/// One client operation sent and not yet ended: started just before its
/// client sends it, [`ended`](Self::ended) once it answered or failed — or
/// dropped unended, its caller having given up, which records it with no
/// failure, as the conventions record a call its caller cancelled. Its span
/// then still exists for whatever a peer was told of it.
///
/// Started outside every unit of work, it is recorded with no span. With
/// nothing recording, it reads no clock and holds nothing.
#[derive(Debug)]
#[must_use = "an operation is recorded when it ends, or when it is dropped"]
pub struct PendingClientOperation {
    timed: Option<Timed>,
}

/// What a pending operation is and when it started: kept only while something
/// records.
#[derive(Debug)]
struct Timed {
    duration: &'static DurationHistogram,
    name: Cow<'static, str>,
    attributes: Vec<(&'static str, Held)>,
    correlation: Option<Correlation>,
    started: SystemTime,
    clock: Instant,
}

/// An attribute's value, held until the operation ends.
#[derive(Debug)]
enum Held {
    Text(Box<str>),
    Integer(i64),
}

impl Held {
    fn of(value: ClientValue<'_>) -> Self {
        match value {
            ClientValue::Text(text) => Self::Text(text.into()),
            ClientValue::Integer(integer) => Self::Integer(integer),
        }
    }

    fn value(&self) -> ClientValue<'_> {
        match self {
            Self::Text(text) => ClientValue::Text(text),
            Self::Integer(integer) => ClientValue::Integer(*integer),
        }
    }
}

impl PendingClientOperation {
    /// The operation `name`, recorded in `duration` and carrying `attributes`,
    /// about to be sent from the unit of work this task runs.
    pub fn start(
        duration: &'static DurationHistogram,
        name: impl Into<Cow<'static, str>>,
        attributes: &[(&'static str, ClientValue<'_>)],
    ) -> Self {
        if !is_recorded() {
            return Self { timed: None };
        }
        let correlation =
            crate::trace_context::__private::current_correlation().map(|unit| unit.child());
        Self::timed(duration, name.into(), attributes, correlation)
    }

    /// The operation, correlated as `correlation`, which a client has told its
    /// peer of.
    fn timed(
        duration: &'static DurationHistogram,
        name: Cow<'static, str>,
        attributes: &[(&'static str, ClientValue<'_>)],
        correlation: Option<Correlation>,
    ) -> Self {
        Self {
            timed: Some(Timed {
                duration,
                name,
                attributes: attributes
                    .iter()
                    .map(|(key, value)| (*key, Held::of(*value)))
                    .collect(),
                correlation,
                started: SystemTime::now(),
                clock: Instant::now(),
            }),
        }
    }

    /// Record the operation as it ended, carrying `attributes` besides those
    /// it started with, failed with the class `failure` names if it failed.
    pub fn ended(mut self, attributes: &[(&'static str, ClientValue<'_>)], failure: Option<&str>) {
        self.record(attributes, failure);
    }

    /// Hand the operation to the recorder, once.
    fn record(&mut self, ended: &[(&'static str, ClientValue<'_>)], failure: Option<&str>) {
        let Some(timed) = self.timed.take() else {
            return;
        };
        let elapsed = timed.clock.elapsed();
        let attributes: Vec<_> = timed
            .attributes
            .iter()
            .map(|(key, held)| (*key, held.value()))
            .chain(ended.iter().copied())
            .collect();
        ClientOperation::new(
            timed.duration,
            &timed.name,
            timed.started,
            elapsed,
            &attributes,
        )
        .correlated(timed.correlation.as_ref())
        .failed(failure)
        .record();
    }
}

/// An operation its caller gave up on before it ended: recorded with no
/// failure.
impl Drop for PendingClientOperation {
    fn drop(&mut self) {
        self.record(&[], None);
    }
}

/// The HTTP conventions' client duration, which every request an
/// [`HttpClientRequest`] times records in.
pub const HTTP_CLIENT_REQUEST_DURATION: DurationHistogram = DurationHistogram::new(
    "http.client.request.duration",
    "Duration of HTTP client requests.",
    REQUEST_BOUNDARIES,
    &[
        MetricAttribute::bounded(HTTP_REQUEST_METHOD),
        MetricAttribute::bounded(SERVER_ADDRESS),
        MetricAttribute::bounded(SERVER_PORT),
        MetricAttribute::bounded(HTTP_RESPONSE_STATUS_CODE),
        MetricAttribute::bounded(NETWORK_PROTOCOL_VERSION),
    ],
);

/// The request's method, from the conventions' known set or `_OTHER`.
const HTTP_REQUEST_METHOD: &str = "http.request.method";
/// The status the server answered.
const HTTP_RESPONSE_STATUS_CODE: &str = "http.response.status_code";
/// The HTTP version the answer came over (`1.1`, `2`).
const NETWORK_PROTOCOL_VERSION: &str = "network.protocol.version";
/// `http` or `https`.
const URL_SCHEME: &str = "url.scheme";
/// How many times a failed request was sent again before this sending.
const HTTP_REQUEST_RESEND_COUNT: &str = "http.request.resend_count";

/// The span name the conventions give a request whose method they do not know.
const UNKNOWN_METHOD_SPAN: &str = "HTTP";

/// The class of a request that never answered because its deadline passed.
const TIMEOUT: &str = "timeout";

/// The W3C header naming the trace and the span that sent the request.
const TRACEPARENT: HeaderName = HeaderName::from_static("traceparent");

/// The W3C header carrying the trace's vendor state, forwarded verbatim.
const TRACESTATE: HeaderName = HeaderName::from_static("tracestate");

/// One HTTP request this process sends, from just before it leaves to its
/// answer: what carries the unit of work's W3C Trace Context onto it and
/// records it as the client operation the HTTP conventions name, whichever
/// client sends it.
///
/// Started inside a unit of work, it names a span of the request's own under
/// the unit: [`announce`](Self::announce) sets the `traceparent` naming it and
/// the unit's `tracestate` verbatim — never `baggage` — and the span recorded
/// for the request is that one. Started outside every unit, it announces
/// nothing. Recorded as a [`PendingClientOperation`] is, once it is
/// [`answered`](Self::answered), [`timed_out`](Self::timed_out) or
/// [`failed`](Self::failed), or dropped: in [`HTTP_CLIENT_REQUEST_DURATION`],
/// by its method, its scheme, its server, its status and the HTTP version it
/// was answered over.
///
/// Never by its path or its query, where a webhook's token or a presigned
/// URL's signature sits. Its server is the conventions' `server.address`,
/// which keys the duration: a client calling hosts its users choose — a
/// webhook's delivery — keys one series per host.
#[derive(Debug)]
pub struct HttpClientRequest {
    announced: Option<Correlation>,
    operation: PendingClientOperation,
}

impl HttpClientRequest {
    /// The request `method` about to leave for `host` on `port` over `scheme`;
    /// a port left out is the scheme's own.
    #[must_use = "a request is recorded when it is answered, timed out or failed"]
    pub fn start(method: &Method, scheme: &str, host: Option<&str>, port: Option<u16>) -> Self {
        let announced =
            crate::trace_context::__private::current_correlation().map(|unit| unit.child());
        if !is_recorded() {
            return Self {
                announced,
                operation: PendingClientOperation { timed: None },
            };
        }
        let method = known_verb(method.as_str()).unwrap_or(OTHER);
        let (scheme, known_port) = match scheme {
            "https" => ("https", Some(443)),
            "http" => ("http", Some(80)),
            _ => (OTHER, None),
        };
        let mut attributes = Vec::with_capacity(4);
        attributes.push((HTTP_REQUEST_METHOD, ClientValue::Text(method)));
        attributes.push((URL_SCHEME, ClientValue::Text(scheme)));
        if let Some(host) = host {
            attributes.push((SERVER_ADDRESS, ClientValue::Text(host)));
        }
        if let Some(port) = port.or(known_port) {
            attributes.push((SERVER_PORT, ClientValue::Integer(i64::from(port))));
        }
        let name = if method == OTHER {
            UNKNOWN_METHOD_SPAN
        } else {
            method
        };
        let operation = PendingClientOperation::timed(
            &HTTP_CLIENT_REQUEST_DURATION,
            Cow::Borrowed(name),
            &attributes,
            announced.clone(),
        );
        Self {
            announced,
            operation,
        }
    }

    /// The request is the `count`th sending again of one that failed: the
    /// conventions' `http.request.resend_count`, on its span alone, and nothing
    /// for a first sending.
    #[must_use = "a request is recorded when it is answered, timed out or failed"]
    pub fn resent(mut self, count: u32) -> Self {
        if count > 0
            && let Some(timed) = &mut self.operation.timed
        {
            timed
                .attributes
                .push((HTTP_REQUEST_RESEND_COUNT, Held::Integer(i64::from(count))));
        }
        self
    }

    /// Set the request's `traceparent` and `tracestate` on `headers`, inside a
    /// unit of work; outside every unit, leave them as they are.
    pub fn announce(&self, headers: &mut HeaderMap) {
        let Some(announced) = &self.announced else {
            return;
        };
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

    /// Record the request as answered `status` over `version`: failed by its
    /// status from `400` up.
    pub fn answered(self, status: StatusCode, version: Version) {
        let failure = (status.is_client_error() || status.is_server_error()).then_some(status);
        let mut attributes = Vec::with_capacity(2);
        attributes.push((
            HTTP_RESPONSE_STATUS_CODE,
            ClientValue::Integer(i64::from(status.as_u16())),
        ));
        if let Some(version) = crate::operation_log::protocol_version(version) {
            attributes.push((NETWORK_PROTOCOL_VERSION, ClientValue::Text(version)));
        }
        self.operation
            .ended(&attributes, failure.as_ref().map(StatusCode::as_str));
    }

    /// Record the request as unanswered within its deadline.
    pub fn timed_out(self) {
        self.operation.ended(&[], Some(TIMEOUT));
    }

    /// Record the request as failed with no answer, for any reason but its
    /// deadline: a refused connection, a handshake, a body that broke.
    pub fn failed(self) {
        self.operation.ended(&[], Some(OTHER));
    }
}

/// What records a client operation once it ended.
type ClientRecorder = fn(&ClientOperation<'_>);

static RECORDER: OnceLock<ClientRecorder> = OnceLock::new();

/// Whether anything records client operations: a client times none when
/// nothing does.
pub fn is_recorded() -> bool {
    RECORDER.get().is_some()
}

/// `client_operation` is public: its tier-2 items live here, reached only
/// through the crate's `__private`.
pub(crate) mod __private {
    use super::{ClientRecorder, RECORDER};

    /// Seed what records every client operation. Called once, at boot, by the
    /// observability stack — never by an application.
    pub fn set_client_recorder(recorder: ClientRecorder) {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "seeded once at boot; a second install keeps the first recorder"
        )]
        let _ = RECORDER.set(recorder);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// What the recorder was handed for one operation: its span's name, its
    /// attributes as written, its class if it failed, and the span it is.
    #[derive(Clone, Debug, PartialEq)]
    struct Recorded {
        name: String,
        attributes: Vec<(&'static str, String)>,
        error_type: Option<String>,
        span: Option<String>,
    }

    static RECORDED: Mutex<Vec<Recorded>> = Mutex::new(Vec::new());

    fn capture(operation: &ClientOperation<'_>) {
        let attributes = operation
            .attributes()
            .iter()
            .map(|(key, value)| match value {
                ClientValue::Text(text) => (*key, (*text).to_owned()),
                ClientValue::Integer(integer) => (*key, integer.to_string()),
            })
            .collect();
        RECORDED.lock().expect("the recorded").push(Recorded {
            name: operation.name().to_owned(),
            attributes,
            error_type: operation.error_type().map(str::to_owned),
            span: operation
                .correlation()
                .map(|correlation| correlation.span_id().to_hex()),
        });
    }

    fn recorded() -> Vec<Recorded> {
        RECORDED.lock().expect("the recorded").clone()
    }

    /// The headers a request started inside `unit`, or outside every unit,
    /// announces over `headers`.
    async fn announced(unit: Option<Correlation>, mut headers: HeaderMap) -> HeaderMap {
        let announce = async {
            HttpClientRequest::start(&Method::GET, "https", Some("api.example"), None)
                .announce(&mut headers);
        };
        match unit {
            Some(unit) => crate::with_request_scope(None, unit, announce).await,
            None => announce.await,
        }
        headers
    }

    #[test]
    fn an_operation_is_failed_by_its_class_and_keeps_the_rest() {
        let started = SystemTime::now();
        let attributes = [(DB_SYSTEM_NAME, ClientValue::Text("postgresql"))];
        let operation = ClientOperation::new(
            &DB_CLIENT_OPERATION_DURATION,
            "SELECT posts",
            started,
            Duration::from_millis(3),
            &attributes,
        );
        assert_eq!(operation.error_type(), None);

        assert_eq!(operation.failed(None).error_type(), None);
        let failed = operation.failed(Some("_OTHER"));
        assert_eq!(failed.error_type(), Some("_OTHER"));
        assert_eq!(failed.name(), "SELECT posts");
        assert_eq!(failed.started(), started);
        assert_eq!(failed.elapsed(), Duration::from_millis(3));
        assert_eq!(failed.attributes(), &attributes);
        assert_eq!(failed.duration(), &DB_CLIENT_OPERATION_DURATION);
        assert!(failed.correlation().is_none());

        let correlation = Correlation::minted(None).child();
        assert_eq!(
            failed
                .correlated(Some(&correlation))
                .correlation()
                .map(Correlation::span_id),
            Some(correlation.span_id()),
        );
    }

    #[test]
    fn with_nothing_recording_an_operation_reads_no_clock() {
        let operation = PendingClientOperation::start(
            &DB_CLIENT_OPERATION_DURATION,
            "GET",
            &[(DB_SYSTEM_NAME, ClientValue::Text("redis"))],
        );
        let request = HttpClientRequest::start(&Method::GET, "https", None, None);

        assert!(operation.timed.is_none());
        assert!(request.operation.timed.is_none());
    }

    #[tokio::test]
    async fn an_operation_is_recorded_once_as_it_ended_or_as_it_was_dropped() {
        __private::set_client_recorder(capture);
        let unit = Correlation::minted(None);
        let start = || {
            PendingClientOperation::start(
                &DB_CLIENT_OPERATION_DURATION,
                "GET",
                &[(DB_SYSTEM_NAME, ClientValue::Text("redis"))],
            )
        };

        crate::with_request_scope(None, unit.clone(), async {
            start().ended(
                &[(DB_NAMESPACE, ClientValue::Integer(2))],
                Some("WRONGTYPE"),
            );
            drop(start());
        })
        .await;
        drop(start());

        let [ended, dropped, outside] = recorded().try_into().expect("three, each once");
        assert_eq!(
            ended.attributes,
            [
                (DB_SYSTEM_NAME, "redis".to_owned()),
                (DB_NAMESPACE, "2".to_owned())
            ]
        );
        assert_eq!(ended.error_type.as_deref(), Some("WRONGTYPE"));
        assert_eq!(
            dropped.error_type, None,
            "an operation given up on failed nothing"
        );
        assert!(
            ended.span.is_some() && dropped.span.is_some() && ended.span != dropped.span,
            "each is a span of its own in the unit's trace",
        );
        assert_ne!(ended.span, Some(unit.span_id().to_hex()));
        assert_eq!(outside.span, None, "outside every unit, no span");
    }

    #[tokio::test]
    async fn a_request_inside_a_unit_announces_a_span_of_its_own_in_the_units_trace() {
        let unit = Correlation::minted(None);

        let headers = announced(Some(unit.clone()), HeaderMap::new()).await;

        let traceparent = headers[TRACEPARENT].to_str().expect("a header value");
        let [version, trace_id, parent_id, _flags] = traceparent
            .split('-')
            .collect::<Vec<_>>()
            .try_into()
            .expect("four fields");
        assert_eq!(version, "00");
        assert_eq!(trace_id, unit.trace_id().to_hex(), "the unit's trace");
        assert_ne!(parent_id, unit.span_id().to_hex(), "a span of its own");
        assert!(!headers.contains_key("baggage"));
        assert!(!headers.contains_key(TRACESTATE), "the unit carries none");
    }

    #[tokio::test]
    async fn a_request_outside_every_unit_leaves_its_headers_as_they_are() {
        let mut set = HeaderMap::new();
        set.insert(TRACEPARENT, HeaderValue::from_static("set-by-the-caller"));

        let headers = announced(None, set.clone()).await;

        assert_eq!(headers, set);
    }

    #[tokio::test]
    async fn a_request_is_recorded_once_as_it_ended_by_its_method_server_and_status() {
        __private::set_client_recorder(capture);
        let unit = Correlation::minted(None);

        let headers = crate::with_request_scope(None, unit, async {
            let start = |method: &Method| {
                HttpClientRequest::start(method, "https", Some("api.example"), None)
            };
            let answered = start(&Method::POST);
            let mut headers = HeaderMap::new();
            answered.announce(&mut headers);
            answered.answered(StatusCode::OK, Version::HTTP_2);
            start(&Method::GET).answered(StatusCode::SERVICE_UNAVAILABLE, Version::HTTP_11);
            start(&Method::GET).timed_out();
            start(&Method::from_bytes(b"PURGE").expect("a method")).failed();
            drop(HttpClientRequest::start(
                &Method::GET,
                "http",
                Some("api.example"),
                Some(8080),
            ));
            headers
        })
        .await;

        let [answered, refused, timed_out, failed, dropped] =
            recorded().try_into().expect("five requests, each once");
        assert_eq!(answered.name, "POST");
        assert_eq!(
            answered.attributes,
            [
                (HTTP_REQUEST_METHOD, "POST".to_owned()),
                (URL_SCHEME, "https".to_owned()),
                (SERVER_ADDRESS, "api.example".to_owned()),
                (SERVER_PORT, "443".to_owned()),
                (HTTP_RESPONSE_STATUS_CODE, "200".to_owned()),
                (NETWORK_PROTOCOL_VERSION, "2".to_owned()),
            ]
        );
        assert_eq!(answered.error_type, None);
        assert_eq!(
            answered.span.as_deref(),
            headers[TRACEPARENT]
                .to_str()
                .ok()
                .and_then(|parent| parent.split('-').nth(2)),
            "the span recorded is the one the peer was told is its parent",
        );
        assert_eq!(refused.error_type.as_deref(), Some("503"));
        assert_eq!(timed_out.error_type.as_deref(), Some(TIMEOUT));
        assert_eq!(failed.name, UNKNOWN_METHOD_SPAN);
        assert_eq!(failed.error_type.as_deref(), Some(OTHER));
        assert!(
            failed
                .attributes
                .contains(&(HTTP_REQUEST_METHOD, OTHER.to_owned()))
        );
        assert_eq!(
            dropped.error_type, None,
            "a request given up on failed nothing"
        );
        assert!(
            !dropped
                .attributes
                .iter()
                .any(|(key, _)| *key == HTTP_RESPONSE_STATUS_CODE)
        );
        assert!(
            dropped
                .attributes
                .contains(&(SERVER_PORT, "8080".to_owned()))
        );
    }
}

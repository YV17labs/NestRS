use std::borrow::Cow;
use std::cell::Cell;
use std::error::Error;
use std::fmt;

use nest_rs_core::__private::{Withheld, find_in_chain};
use nest_rs_core::{Code, DecodeError, Failure, OPAQUE_CLIENT_MESSAGE, Problem, ToProblem, code};
use nest_rs_pipes::PipeError;
use serde::{Deserialize, Serialize};

use crate::opaque::Opaque;

/// What an event no handler serves is answered.
const UNKNOWN_EVENT: &str = "unknown event";

/// What a text message that is no envelope is answered, before where it failed.
const INVALID_ENVELOPE: &str = "invalid envelope";

/// `text` decoded as the envelope every message rides, or the error saying
/// where in it the decode failed, never the value.
pub(crate) fn decode_envelope(text: &str) -> Result<WsEnvelope, WsError> {
    serde_json::from_str(text).map_err(|error| {
        let report = DecodeError::new(&error)
            .located::<WsEnvelope, _>(&mut serde_json::Deserializer::from_str(text));
        WsError::invalid_argument(match report.path() {
            Some(path) => format!("{INVALID_ENVELOPE} at `{path}`: {report}"),
            None => format!("{INVALID_ENVELOPE}: {report}"),
        })
    })
}

/// `data` decoded as the payload of `event`'s handler, or the error frame
/// saying where in it the decode failed ([`WsReply::payload_error`]).
pub fn decode_payload<T: serde::de::DeserializeOwned>(
    event: &str,
    data: &serde_json::Value,
) -> Result<T, WsReply> {
    T::deserialize(data).map_err(|error| {
        WsReply::payload_error(event, DecodeError::new(&error).located::<T, _>(data))
    })
}

/// `{ "event": ..., "data": ... }` — the wire shape every gateway message
/// rides.
///
/// It decodes from a JSON object alone: anything else, an array included, is
/// refused as no envelope, never read by position
/// (`.claude/decisions/wire-struct-never-by-position.md`).
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct WsEnvelope {
    /// The message's event name, routing it and labelling its reply.
    pub event: String,
    /// The handler payload. Defaults to `null` when the frame omits `data`, so
    /// an argument-less event decodes without an explicit field.
    pub data: serde_json::Value,
}

/// The envelope's fields, by the names its wire gives them: the names
/// [`DecodeError::located`] places a failure at.
const ENVELOPE_FIELDS: &[&str] = &["event", "data"];

impl<'de> Deserialize<'de> for WsEnvelope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct("WsEnvelope", ENVELOPE_FIELDS, EnvelopeVisitor)
    }
}

/// Reads an envelope from an object, and from nothing else.
struct EnvelopeVisitor;

impl<'de> serde::de::Visitor<'de> for EnvelopeVisitor {
    type Value = WsEnvelope;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object with `event` and `data`")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<WsEnvelope, A::Error> {
        let EnvelopeObject { event, data } =
            EnvelopeObject::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
        Ok(WsEnvelope { event, data })
    }
}

/// The envelope's object, its fields read by serde's derive once the frame is
/// known to be one.
#[derive(Deserialize)]
struct EnvelopeObject {
    event: String,
    #[serde(default)]
    data: serde_json::Value,
}

impl WsEnvelope {
    /// Serialize `data` into the `{ event, data }` wire frame for a given event
    /// name, without an intermediate `Value`.
    pub fn encode<T: Serialize + ?Sized>(
        event: &str,
        data: &T,
    ) -> Result<String, serde_json::Error> {
        #[derive(Serialize)]
        struct Frame<'a, T: ?Sized> {
            event: &'a str,
            data: &'a T,
        }
        serde_json::to_string(&Frame { event, data })
    }
}

/// What an error frame carries: the human-readable message, the class a client
/// branches on, a problem's RFC 9457 type and title, the evidence a refusal
/// names beside them, and a field map under HTTP's RFC 9457 `errors` name.
///
/// Each member is the frame's `data.<member>`, omitted when there is none, as
/// HTTP omits it: `{ "error": …, "code"?: …, "type"?: …, "title"?: …,
/// "reason"?: …, "requiredScopes"?: […], "retryAfterSeconds"?: …, "errors"?:
/// { … } }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WsError {
    /// The human-readable message — the frame's `data.error`.
    pub error: String,
    /// The class a client branches on (`nest_rs_core::code`) — the
    /// frame's `data.code`; `None` on a frame a handler built with
    /// [`new`](Self::new), which names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Cow<'static, str>>,
    /// The problem's RFC 9457 type, when it named one: the frame's
    /// `data.type` and `data.title`. Boxed, as `errors` is.
    #[serde(flatten)]
    pub problem_type: Option<Box<WsProblemType>>,
    /// A refusal's class in lowercase words, the member these frames carried
    /// first (`forbidden`, `timeout`) — the frame's `data.reason`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Cow<'static, str>>,
    /// The scopes a scope denial requires (RFC 6750 §3.1) — the frame's
    /// `data.requiredScopes`. A boxed slice, as `errors` is boxed.
    #[serde(default, skip_serializing_if = "<[String]>::is_empty")]
    pub required_scopes: Box<[String]>,
    /// The wait before a retry, in seconds — the frame's
    /// `data.retryAfterSeconds`, every edge's spelling of it.
    #[serde(
        default,
        rename = "retryAfterSeconds",
        skip_serializing_if = "Option::is_none"
    )]
    pub retry_after_secs: Option<u32>,
    /// A field map, field by field, as every edge carries a pipe's or a
    /// problem's — the frame's `data.errors`, and nothing else is. Boxed, so the `Err`
    /// of every handler's `Result` stays the size of the members most frames
    /// carry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errors: Option<Box<serde_json::Value>>,
    /// Whether the frame answers a failure of the server's own, a `5xx`'s
    /// class: what the message's span records as an error. Never on the wire.
    #[serde(skip)]
    pub(crate) server_failed: bool,
}

impl WsError {
    /// A message with no class and no field-level errors.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
            code: None,
            problem_type: None,
            reason: None,
            required_scopes: Box::default(),
            retry_after_secs: None,
            errors: None,
            server_failed: false,
        }
    }

    /// This frame with `errors`, the field-level errors a client acts on per
    /// field, as a pipe's [`details`](PipeError::details) carry them.
    ///
    /// ```
    /// use nest_rs_ws::WsError;
    ///
    /// let frame = WsError::new("the name is taken")
    ///     .with_errors(serde_json::json!({ "name": ["taken"] }));
    /// assert_eq!(frame.errors.as_deref(), Some(&serde_json::json!({ "name": ["taken"] })));
    /// ```
    pub fn with_errors(mut self, errors: serde_json::Value) -> Self {
        self.errors = Some(Box::new(errors));
        self
    }

    /// The frame a [`Problem`] answers: its
    /// [`client_message`](Problem::client_message), its code, and its type,
    /// its title, its wait as `retryAfterSeconds` and its field map as
    /// `errors` when it has them.
    ///
    /// ```
    /// use nest_rs_core::{Problem, code};
    /// use nest_rs_ws::WsError;
    ///
    /// let frame = WsError::from_problem(&Problem::new(409, code::CONFLICT));
    /// assert_eq!(
    ///     serde_json::to_value(&frame).unwrap(),
    ///     serde_json::json!({ "error": "conflict", "code": "CONFLICT" }),
    /// );
    /// ```
    pub fn from_problem(problem: &Problem) -> Self {
        Self {
            problem_type: problem.type_uri().zip(problem.title()).map(|(uri, title)| {
                Box::new(WsProblemType {
                    uri: Cow::Borrowed(uri),
                    title: Cow::Borrowed(title),
                })
            }),
            retry_after_secs: problem.retry_after(),
            errors: problem.errors().cloned().map(Box::new),
            server_failed: problem.is_server_error(),
            ..Self::classed(problem.client_message(), problem.code())
        }
    }

    /// `message`, under `class`.
    pub(crate) fn classed(message: impl Into<String>, class: Code) -> Self {
        Self {
            code: Some(Cow::Borrowed(class.as_str())),
            ..Self::new(message)
        }
    }

    /// This frame with `reason`, its class in lowercase words.
    pub(crate) fn with_reason(mut self, reason: &'static str) -> Self {
        self.reason = Some(Cow::Borrowed(reason));
        self
    }

    /// The frame every failure the client is not told of answers, whoever
    /// answered it: the `INTERNAL` class's, [`OPAQUE_CLIENT_MESSAGE`] under its
    /// code.
    pub(crate) fn internal() -> Self {
        Self::from_problem(&Problem::new(500, code::INTERNAL))
    }

    /// The frame an event no handler serves answers, so a client can tell a
    /// typo from an auth failure: the `NOT_FOUND` class HTTP's `404` names.
    /// The frame answers on the event it sent, and neither its text nor the
    /// report repeats it, the event being only the client's word. The report
    /// is at `debug`: the client's error, which the message's line files under
    /// `_OTHER`.
    pub(crate) fn unknown_event() -> Self {
        tracing::debug!(
            target: crate::TARGET,
            path = crate::gateway::served_path().as_deref(),
            event = nest_rs_core::operation_log::OTHER,
            "subscribe_message dispatched to an unknown event",
        );
        Self::classed(UNKNOWN_EVENT, code::NOT_FOUND)
    }

    /// The frame a value that does not decode answers: `sentence`, under the
    /// `INVALID_ARGUMENT` code HTTP's `400` names for a body that does not.
    pub(crate) fn invalid_argument(sentence: impl Into<String>) -> Self {
        Self::classed(sentence, code::INVALID_ARGUMENT)
    }

    /// The frame a [`PipeError`] answers: `message`, under the
    /// `INVALID_ARGUMENT` code HTTP's `400` names for it, and its field-level
    /// `errors` when it has some.
    fn piped(message: impl Into<String>, errors: Option<serde_json::Value>) -> Self {
        Self {
            errors: errors.map(Box::new),
            ..Self::invalid_argument(message)
        }
    }
}

/// A problem's RFC 9457 type as an error frame carries it, beside its code:
/// both members or neither, as `Problem::with_type` sets them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WsProblemType {
    /// The URI naming the type (RFC 9457 §3.1.1) — the frame's `data.type`.
    #[serde(rename = "type")]
    pub uri: Cow<'static, str>,
    /// The type's summary (RFC 9457 §3.1.3) — the frame's `data.title`.
    pub title: Cow<'static, str>,
}

impl std::fmt::Display for WsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error)
    }
}

/// A handler may return it as its `Err`; [`WsReply::from_handler_error`] sends
/// it whole, details included.
impl std::error::Error for WsError {}

impl<T: Into<String>> From<T> for WsError {
    fn from(message: T) -> Self {
        Self::new(message)
    }
}

/// What a message answers: a frame, an error frame, or silence.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum WsReply {
    /// Send this value back on the request's event name.
    Reply(serde_json::Value),
    /// Send nothing — the handler returned `()` or chose to stay silent.
    None,
    /// Send an error frame `{ event, data: { error, code?, …, errors? } }`.
    Error(WsError),
}

impl WsReply {
    /// Serializes a handler's return; a failure degrades to an [`Opaque`]
    /// [`WsReply::Error`], since serde's `Display` can carry the value.
    pub fn reply<T: Serialize>(value: &T) -> WsReply {
        use nest_rs_core::__private::{AnswerRefusal, take_refused_answer};
        let _ = take_refused_answer();
        let written = serde_json::to_value(value);
        // A wire row the caller may not read refused itself, and said so: a denial.
        if written.is_err() && take_refused_answer() == Some(AnswerRefusal::Withheld) {
            return WsReply::Error(WsError::from_problem(&nest_rs_core::Problem::new(
                403,
                nest_rs_core::code::FORBIDDEN,
            )));
        }
        match written.opaque() {
            Ok(data) => WsReply::Reply(data),
            Err(err) => WsReply::Error(err),
        }
    }

    /// Build an [`Error`](WsReply::Error) reply from an arbitrary message.
    pub fn error(message: impl Into<String>) -> WsReply {
        WsReply::Error(WsError::new(message))
    }

    /// The error frame a rejected [`PipeError`] puts on the wire, under the
    /// `INVALID_ARGUMENT` code with its field-level errors beside it, reported
    /// at `debug` on `nest_rs::ws`: the client's error, which the message's
    /// line files.
    pub fn pipe_error(event: &str, what: &str, error: nest_rs_pipes::PipeError) -> WsReply {
        let message = format!("invalid {what} for `{event}`: {}", error.message());
        tracing::debug!(
            target: crate::TARGET,
            path = crate::gateway::served_path().as_deref(),
            event,
            kind = what,
            error = error.message(),
            "subscribe_message rejected by a pipe",
        );
        WsReply::Error(WsError::piped(message, error.into_details()))
    }

    /// The error frame a payload that does not deserialize puts on the wire,
    /// under the `INVALID_ARGUMENT` code, reported at `debug` on `nest_rs::ws`:
    /// the client's error, which the message's line files. Both say where it
    /// failed — inside the payload when the report is
    /// [`located`](DecodeError::located) — what kind of value was found and
    /// what was expected ([`DecodeError`]), never the value.
    pub fn payload_error(event: &str, error: impl Into<DecodeError>) -> WsReply {
        let error = error.into();
        let said = match error.path() {
            Some(path) => format!("at `{path}`: {error}"),
            None => error.to_string(),
        };
        tracing::debug!(
            target: crate::TARGET,
            path = crate::gateway::served_path().as_deref(),
            event,
            error = %said,
            "subscribe_message payload failed to deserialize",
        );
        let message = match error.path() {
            Some(path) => format!("invalid payload for `{event}` at `{path}`: {error}"),
            None => format!("invalid payload for `{event}`: {error}"),
        };
        WsReply::Error(WsError::invalid_argument(message))
    }

    /// The reply for an event with no registered handler: the `NOT_FOUND`
    /// frame, on the event it sent.
    pub fn unknown() -> WsReply {
        WsReply::Error(WsError::unknown_event())
    }

    /// The error frame a handler's `Err` produces, read off the vocabulary its
    /// chain carries: a [`WsError`] is sent whole, a [`Problem`] in its frame
    /// ([`WsError::from_problem`]), a [`PipeError`] with its detail, a decode
    /// failure without its value under the `INVALID_ARGUMENT` code — each a
    /// refusal the message's line files, with nothing beside it.
    /// Anything else answers [`OPAQUE_CLIENT_MESSAGE`] under the `INTERNAL`
    /// code, its chain filed once at `error`, as is the chain behind a `5xx`
    /// problem — at `warn` behind a `503`.
    pub fn from_handler_error<E>(event: &str, error: E) -> WsReply
    where
        E: Into<Box<dyn Error + Send + Sync>> + 'static,
    {
        chain_rendered(&*nest_rs_core::boxed_error(error)).into_reply(event)
    }
}

/// An error frame, and what it keeps from the client, filed for the operator
/// once the unit's answer is final: an answer an exception filter replaced
/// files nothing of what it replaced.
#[derive(Debug)]
pub(crate) struct Rendered {
    /// What the client reads.
    pub(crate) frame: WsError,
    /// The chain the frame withholds, filed once the frame goes out; `None`
    /// for a refusal, which the message's `ws.message` line alone files.
    pub(crate) withheld: Option<Withheld>,
}

impl Rendered {
    /// The frame, what it withholds filed at once for the message `event`
    /// declares, at the gateway the running socket is served at.
    fn into_reply(self, event: &str) -> WsReply {
        if let Some(withheld) = self.withheld {
            file(withheld, crate::gateway::served_path().as_deref(), event);
        }
        WsReply::Error(self.frame)
    }
}

/// File the chain a frame answering `event`'s message, at the gateway served at
/// `path`, withholds: behind a `503`, the server unable to serve it in time, at
/// `warn`; behind any other failure, at `error`.
pub(crate) fn file(withheld: Withheld, path: Option<&str>, event: &str) {
    match withheld {
        Withheld::Unavailable(said) => tracing::warn!(
            target: crate::TARGET,
            path,
            event,
            error = %said,
            "websocket message unavailable",
        ),
        Withheld::Failed(said) => tracing::error!(
            target: crate::TARGET,
            path,
            event,
            error = %said,
            "websocket message failed",
        ),
    }
}

/// The frame a failure's chain carries, else the opaque one. A frame the
/// chain says — a [`WsError`], a client [`Problem`], a [`PipeError`], a
/// decode failure — is a refusal and withholds nothing.
pub(crate) fn chain_rendered(error: &(dyn Error + 'static)) -> Rendered {
    let frame = if let Some(frame) = find_in_chain::<WsError>(error) {
        frame.clone()
    } else if let Some(problem) = find_in_chain::<Problem>(error) {
        return problem_rendered(error, problem);
    } else if let Some(rejection) = find_in_chain::<PipeError>(error) {
        WsError::piped(rejection.message(), rejection.details().cloned())
    } else if let Some(report) = DecodeError::in_chain(error) {
        WsError::invalid_argument(report.to_string())
    } else {
        return opaque_rendered(nest_rs_core::error_message(error));
    };
    Rendered {
        frame,
        withheld: None,
    }
}

/// `problem`'s frame as the answer to `error`, whoever answered it — the
/// handler, or an exception filter in its place: only the chain a `5xx`
/// withholds is filed, so a refusal, and a bare problem, file nothing.
pub(crate) fn problem_rendered(error: &(dyn Error + 'static), problem: &Problem) -> Rendered {
    Rendered {
        frame: WsError::from_problem(problem),
        withheld: nest_rs_core::__private::withheld(problem, error),
    }
}

/// The opaque frame, with what the error said on the operator's line at `error`.
fn opaque_rendered(said: String) -> Rendered {
    Rendered {
        frame: WsError::internal(),
        withheld: Some(Withheld::Failed(said)),
    }
}

/// A handler's error, rendered by its type: the frame it answers, what that
/// frame withholds, filed once it goes out, and the failure the exception
/// filters read — the error itself, whenever it can cross threads.
pub struct HandlerFailed {
    rendered: Rendered,
    failure: Failure,
}

impl HandlerFailed {
    /// `rendered`, failing the unit with `failure`.
    fn new(rendered: Rendered, failure: Failure) -> Self {
        Self { rendered, failure }
    }

    /// The frame, what it withholds filed at once for the message `event`
    /// declares: what a handler answers called on its own.
    pub fn into_reply(self, event: &str) -> WsReply {
        self.rendered.into_reply(event)
    }

    /// The rendered frame, and the failure it answers.
    pub(crate) fn into_parts(self) -> (Rendered, Failure) {
        (self.rendered, self.failure)
    }
}

impl fmt::Debug for HandlerFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HandlerFailed")
            .field("frame", &self.rendered.frame)
            .field("failure", &self.failure)
            .finish()
    }
}

/// What the exception filters read of a handler's error that cannot cross
/// threads: its chain, as a sentence.
#[derive(Debug)]
struct Unsendable(String);

impl fmt::Display for Unsendable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for Unsendable {}

/// `rendered`, leaving the exception filters `said`: the chain of a handler's
/// error that cannot cross threads, whatever its frame files.
fn unsendable(rendered: Rendered, said: String) -> HandlerFailed {
    HandlerFailed::new(rendered, Failure::from(Unsendable(said)))
}

/// Turns a handler's return value into a [`WsReply`] **by type**, so a `Result`
/// behind an alias (`type ServiceResult<T> = Result<T, MyError>`) still fails
/// rather than serializing its `Err` into a success frame.
///
/// The inherent method applies to any `Result<T, E>` and wins over the blanket
/// trait method; it carries no bound on `E`, since a failed bound would drop an
/// `Err` onto the blanket method and serialize it whole.
///
/// The expansion splits twice, so `Result<Result<T, E1>, E2>` fails on either
/// `Err`; a `Result` any deeper, or inside an `Option`, a `Vec` or a struct, is
/// data.
pub struct ReplyValue<T>(pub T);

/// A handler's return value, split into what replies and what failed.
///
/// Exhaustive on purpose: a return replies or it failed, so a caller matches
/// both and never needs a `_` arm.
#[expect(
    clippy::exhaustive_enums,
    reason = "closed by definition: a return replies or it failed, and every `#[messages]` \
              expansion matches both"
)]
pub enum ReplyOutcome<T, E> {
    /// The value a success replies with — split again, or serialized.
    Value(T),
    /// The error an `Err` carried, already in the [`ErrorReport`] that turns it
    /// into a frame, so an `Infallible` one adds no unreachable call to the expansion.
    Failed(ErrorReport<E>),
}

impl<T, E> ReplyValue<Result<T, E>> {
    /// A `Result` however it was spelled: `Ok` is the value, `Err` is handed back.
    pub fn into_outcome(self) -> ReplyOutcome<T, E> {
        match self.0 {
            Ok(value) => ReplyOutcome::Value(value),
            Err(err) => ReplyOutcome::Failed(ErrorReport::new(err)),
        }
    }
}

/// The ordinary case: any other value is the value. A trait, so the inherent
/// `Result` impl above is probed first.
pub trait ReplyValueFallback {
    /// The value itself.
    type Value;

    /// The value, with nothing that can have failed.
    fn into_outcome(self) -> ReplyOutcome<Self::Value, std::convert::Infallible>;
}

impl<T> ReplyValueFallback for ReplyValue<T> {
    type Value = T;

    fn into_outcome(self) -> ReplyOutcome<T, std::convert::Infallible> {
        ReplyOutcome::Value(self.0)
    }
}

/// A handler's error, turned into the frame a client reads, what that frame
/// withholds, filed for the operator once it goes out, and the failure the
/// exception filters read — **by type**, resolved where the expansion names
/// it, in four tiers, each taken only when the one above does not apply:
///
/// 1. A [`ToProblem`] error takes the inherent method: the problem it says, in
///    its frame ([`WsError::from_problem`]), a refusal filing nothing beside
///    the message's line; `None` answers opaquely, its chain filed at `error`,
///    as is the chain behind a `5xx` problem — at `warn` behind a `503`.
/// 2. Any other error that crosses threads takes [`ErrorReportChain`]: the
///    vocabulary its chain carries ([`WsReply::from_handler_error`]), else the
///    opaque frame.
/// 3. An error that does not cross threads takes [`ErrorReportLocal`]: the same
///    frame, but an exception filter reads its chain as a sentence, never its
///    type.
/// 4. Any other `Display` type takes [`ErrorReportFallback`]: it has no causes
///    to read vocabulary from, so it answers opaquely, its sentence on the
///    operator's line said without a decode failure's value.
///
/// A type that is none of them does not compile. The error sits in a [`Cell`]
/// so that the tiers reached through a borrow still take it.
pub struct ErrorReport<E>(Cell<Option<E>>);

impl<E> ErrorReport<E> {
    /// The report of a handler's `error`.
    pub fn new(error: E) -> Self {
        Self(Cell::new(Some(error)))
    }

    /// The error, which only the tier that renders it takes.
    fn take(&self) -> Option<E> {
        self.0.take()
    }
}

impl<E: ToProblem> ErrorReport<E> {
    /// The problem's frame, or the opaque one, failing with the error itself.
    pub fn into_failure(self) -> HandlerFailed {
        let Some(error) = self.0.into_inner() else {
            return taken();
        };
        let rendered = match error.to_problem() {
            Some(problem) => problem_rendered(&error, &problem),
            None => opaque_rendered(nest_rs_core::error_message(&error)),
        };
        HandlerFailed::new(rendered, Failure::new(error))
    }
}

/// The error that is not a [`ToProblem`] case of [`ErrorReport`] and crosses
/// threads — the second tier.
pub trait ErrorReportChain {
    /// The frame its chain's vocabulary answers, else the opaque one, failing
    /// with the error itself.
    fn into_failure(self) -> HandlerFailed;
}

impl<E> ErrorReportChain for ErrorReport<E>
where
    E: Into<Box<dyn Error + Send + Sync>> + 'static,
{
    fn into_failure(self) -> HandlerFailed {
        let Some(error) = self.0.into_inner() else {
            return taken();
        };
        let error = nest_rs_core::boxed_error(error);
        let rendered = chain_rendered(&*error);
        HandlerFailed::new(rendered, Failure::new(error))
    }
}

/// The error that does not cross threads — the third tier, implemented on a
/// borrow so that method resolution reaches it only after the two above.
pub trait ErrorReportLocal {
    /// The frame its chain's vocabulary answers, else the opaque one, failing
    /// with its chain as a sentence.
    fn into_failure(self) -> HandlerFailed;
}

impl<E> ErrorReportLocal for &ErrorReport<E>
where
    E: Into<Box<dyn Error>> + 'static,
{
    fn into_failure(self) -> HandlerFailed {
        let Some(error) = self.take() else {
            return taken();
        };
        let error = boxed(error);
        let said = nest_rs_core::error_message(&*error);
        unsendable(chain_rendered(&*error), said)
    }
}

/// `error` boxed with every link kept as the type it is: an `anyhow::Error` is
/// unwrapped as [`nest_rs_core::boxed_error`] unwraps it, since anyhow's own
/// box hides the error it holds from a chain walk.
fn boxed<E: Into<Box<dyn Error>> + 'static>(error: E) -> Box<dyn Error> {
    let mut slot = Some(error);
    if let Some(anyhow) = (&mut slot as &mut dyn std::any::Any)
        .downcast_mut::<Option<nest_rs_core::anyhow::Error>>()
        .and_then(Option::take)
    {
        return anyhow.reallocate_into_boxed_dyn_error_without_backtrace();
    }
    // Unreachable fallback: `slot` is emptied only by the `take` that returned above.
    slot.map_or_else(|| Box::from(OPAQUE_CLIENT_MESSAGE), Into::into)
}

/// The `Display`-only case of [`ErrorReport`] — the fourth tier, implemented on
/// a mutable borrow so that method resolution reaches it only after the three
/// above.
pub trait ErrorReportFallback {
    /// The opaque frame, with the error's sentence on the operator's line.
    fn into_failure(self) -> HandlerFailed;
}

/// A `Display`-only error has no chain to read a decode failure off, so its
/// sentence is read by serde's wording alone.
impl<E: fmt::Display> ErrorReportFallback for &mut ErrorReport<E> {
    fn into_failure(self) -> HandlerFailed {
        let Some(error) = self.take() else {
            return taken();
        };
        let said = DecodeError::redact(&error.to_string(), None).into_owned();
        unsendable(opaque_rendered(said.clone()), said)
    }
}

/// What a report answers once a tier took its error: unreachable, since each
/// call takes the report whole.
fn taken() -> HandlerFailed {
    let said = "a handler's error was rendered twice";
    unsendable(opaque_rendered(said.to_owned()), said.to_owned())
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;

    #[derive(Serialize)]
    struct Hello {
        msg: &'static str,
    }

    #[test]
    fn encode_emits_envelope_shape() {
        let frame = WsEnvelope::encode("chat:say", &Hello { msg: "hi" }).expect("encode");
        let json: serde_json::Value = serde_json::from_str(&frame).expect("parse");
        assert_eq!(json["event"], "chat:say");
        assert_eq!(json["data"]["msg"], "hi");
    }

    #[test]
    fn decode_treats_missing_data_as_null() {
        let env: WsEnvelope = serde_json::from_str(r#"{"event":"ping"}"#).expect("decode");
        assert_eq!(env.event, "ping");
        assert!(env.data.is_null(), "missing data defaults to null");
    }

    #[test]
    fn reply_carries_serialized_value() {
        match WsReply::reply(&Hello { msg: "ok" }) {
            WsReply::Reply(value) => assert_eq!(value["msg"], "ok"),
            _ => panic!("expected Reply"),
        }
    }

    /// The event is the client's word: the frame answers on it, its text
    /// repeats none of it, and it names the class HTTP's `404` names.
    #[test]
    fn unknown_event_message_names_no_event() {
        match WsReply::unknown() {
            WsReply::Error(err) => assert_eq!(
                serde_json::to_value(&err).expect("a frame"),
                serde_json::json!({ "error": UNKNOWN_EVENT, "code": "NOT_FOUND" }),
            ),
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn error_constructor_carries_the_message() {
        match WsReply::error("boom") {
            WsReply::Error(err) => {
                assert_eq!(err.error, "boom");
                assert!(err.errors.is_none(), "no structured detail to give");
            }
            _ => panic!("expected Error"),
        }
    }

    /// A pipe's refusal names its class beside its field map, as HTTP's
    /// problem document and GraphQL's extensions do, whichever tier read it;
    /// with no field map, the class alone.
    #[test]
    fn pipe_error_carries_its_class_beside_the_field_level_detail() {
        let _logs = nest_rs_testing::LogCapture::install();
        let details = serde_json::json!({ "text": [{ "code": "length" }] });
        let detailed = || PipeError::with_details("validation failed", details.clone());

        let WsReply::Error(piped) = WsReply::pipe_error("validated", "payload", detailed()) else {
            panic!("a pipe's refusal answers an error frame");
        };
        assert_eq!(
            serde_json::to_value(&piped).expect("a frame"),
            serde_json::json!({
                "error": "invalid payload for `validated`: validation failed",
                "code": "INVALID_ARGUMENT",
                "errors": { "text": [{ "code": "length" }] },
            }),
        );
        assert_eq!(
            serde_json::to_value(chain_rendered(&detailed()).frame).expect("a frame"),
            serde_json::json!({
                "error": "validation failed",
                "code": "INVALID_ARGUMENT",
                "errors": { "text": [{ "code": "length" }] },
            }),
        );

        let WsReply::Error(bare) =
            WsReply::pipe_error("send", "payload", PipeError::new("must be a valid UUID"))
        else {
            panic!("a pipe's refusal answers an error frame");
        };
        assert_eq!(
            serde_json::to_value(&bare).expect("a frame"),
            serde_json::json!({
                "error": "invalid payload for `send`: must be a valid UUID",
                "code": "INVALID_ARGUMENT",
            }),
        );
    }

    /// A value that does not decode names the class HTTP's `400` names under
    /// `errors`, its sentence kept and its value never said, whichever tier
    /// read it: the payload's own decode, or a decode failure a handler's chain
    /// carries.
    #[test]
    fn a_decode_failure_names_the_invalid_argument_class() {
        let _logs = nest_rs_testing::LogCapture::install();
        let error = serde_json::from_str::<u64>(r#""sk_live_secret""#).expect_err("not a number");

        let WsReply::Error(payload) = WsReply::payload_error("named", &error) else {
            panic!("a payload that does not decode answers an error frame");
        };
        let carried = chain_rendered(&error).frame;

        for frame in [payload, carried] {
            let said = serde_json::to_value(&frame).expect("a frame");
            assert_eq!(said["code"], "INVALID_ARGUMENT", "{said}");
            assert!(said.get("errors").is_none(), "no field map: {said}");
            assert!(frame.error.contains("invalid type"), "{frame:?}");
            assert!(!frame.error.contains("sk_live"), "{frame:?}");
        }
    }

    #[test]
    fn an_error_without_detail_omits_the_errors_member() {
        let frame = serde_json::to_value(WsError::new("nope")).expect("serialize");
        assert_eq!(frame["error"], "nope");
        assert!(
            frame.get("errors").is_none(),
            "absent detail must not ship an explicit null, matching HTTP: {frame}",
        );
    }

    /// A problem's RFC 9457 type and title, and its field map, ride in the
    /// frame's `data` beside its code; a problem naming none carries none.
    #[test]
    fn a_typed_problem_carries_its_type_title_and_field_map() {
        use nest_rs_core::code;

        let taken = Problem::new(409, code::CONFLICT)
            .with_type("https://example.com/problems/handle-taken", "Handle taken")
            .with_detail("@ada is taken")
            .with_errors(serde_json::json!({ "handle": [{ "code": "taken" }] }));
        assert_eq!(
            serde_json::to_value(WsError::from_problem(&taken)).expect("a frame"),
            serde_json::json!({
                "error": "@ada is taken",
                "code": "CONFLICT",
                "type": "https://example.com/problems/handle-taken",
                "title": "Handle taken",
                "errors": { "handle": [{ "code": "taken" }] },
            }),
        );
        assert_eq!(
            serde_json::to_value(WsError::from_problem(&Problem::new(404, code::NOT_FOUND)))
                .expect("a frame"),
            serde_json::json!({ "error": "not found", "code": "NOT_FOUND" }),
        );
    }

    /// Every member reads and writes under the name every edge gives it, so a
    /// client decoding a frame with `WsError` reads what the server wrote.
    #[test]
    fn a_frame_round_trips_through_its_wire_names() {
        let wire = serde_json::json!({
            "error": "slow down",
            "code": "RATE_LIMITED",
            "type": "https://example.com/problems/slow-down",
            "title": "Slow down",
            "reason": "rate_limited",
            "requiredScopes": ["posts:write"],
            "retryAfterSeconds": 7,
            "errors": { "text": ["too long"] },
        });
        let frame: WsError = serde_json::from_value(wire.clone()).expect("a frame");
        assert_eq!(frame.code.as_deref(), Some("RATE_LIMITED"));
        let problem_type = frame.problem_type.as_deref().expect("a typed frame");
        assert_eq!(problem_type.uri, "https://example.com/problems/slow-down");
        assert_eq!(problem_type.title, "Slow down");
        assert_eq!(frame.reason.as_deref(), Some("rate_limited"));
        assert_eq!(*frame.required_scopes, ["posts:write"]);
        assert_eq!(frame.retry_after_secs, Some(7));
        assert_eq!(
            frame.errors.as_deref(),
            Some(&serde_json::json!({ "text": ["too long"] }))
        );
        assert_eq!(serde_json::to_value(&frame).expect("a frame"), wire);
    }

    #[derive(Debug)]
    struct Outage;

    impl std::fmt::Display for Outage {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the search index at 10.0.0.1 refused")
        }
    }

    impl Error for Outage {}

    impl ToProblem for Outage {
        fn to_problem(&self) -> Option<Problem> {
            Some(Problem::new(503, nest_rs_core::code::UNAVAILABLE).with_retry_after(30))
        }
    }

    /// Says nothing it may tell a client.
    #[derive(Debug)]
    struct Silent;

    impl std::fmt::Display for Silent {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the ledger at 10.0.0.1 holds secret 42")
        }
    }

    impl Error for Silent {}

    impl ToProblem for Silent {
        fn to_problem(&self) -> Option<Problem> {
            None
        }
    }

    /// A `Display`-only error, which no tier above the last one takes.
    struct Worded;

    impl std::fmt::Display for Worded {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the ledger at 10.0.0.1 holds secret 42")
        }
    }

    /// Whichever tier renders a handler's error, the failure it does not
    /// describe answers the one opaque frame — the `INTERNAL` class's — as
    /// the class's own problem does.
    #[test]
    fn every_opaque_answer_names_the_internal_class() {
        use nest_rs_core::code;

        let _logs = nest_rs_testing::LogCapture::install();
        let frames = [
            ErrorReport::new(Silent).into_failure().into_reply("charge"),
            ErrorReportChain::into_failure(ErrorReport::new(std::io::Error::other("secret 42")))
                .into_reply("charge"),
            ErrorReportFallback::into_failure(&mut ErrorReport::new(Worded)).into_reply("charge"),
            WsReply::from_handler_error("charge", std::io::Error::other("secret 42")),
            WsReply::from_handler_error("charge", Problem::new(500, code::INTERNAL)),
        ];
        for frame in frames {
            let WsReply::Error(frame) = frame else {
                panic!("an error answers an error frame");
            };
            crate::testing::assert_opaque(&frame);
        }
    }

    /// A server problem answers the sentence an opaque failure answers, so it
    /// files the line an opaque failure files, at `error`, by either tier, and
    /// a `503` the chain behind it at `warn`; a bare problem or a client one
    /// withholds nothing, and files nothing.
    #[test]
    fn a_server_problem_files_the_chain_its_frame_withholds() {
        use nest_rs_core::code;

        let logs = nest_rs_testing::LogCapture::install();
        let failed = || logs.find(crate::TARGET, "websocket message failed");
        let answered = || logs.find(crate::TARGET, "websocket message unavailable");

        let WsReply::Error(frame) = ErrorReport::new(Outage).into_failure().into_reply("search")
        else {
            panic!("an error answers an error frame");
        };
        assert_eq!(
            serde_json::to_value(&frame).expect("a frame"),
            serde_json::json!({
                "error": nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE,
                "code": "UNAVAILABLE",
                "retryAfterSeconds": 30,
            }),
        );
        let unavailable = answered();
        assert_eq!(unavailable.len(), 1, "{unavailable:#?}");
        assert_eq!(
            unavailable[0].level, "warn",
            "a 503 is the server unable to serve it in time"
        );
        assert!(
            unavailable[0]
                .field("error")
                .is_some_and(|e| e.contains("10.0.0.1")),
            "{unavailable:#?}"
        );
        assert!(failed().is_empty(), "{:#?}", failed());

        let carried = nest_rs_core::anyhow::Error::from(Problem::new(500, code::INTERNAL))
            .context("charging the card");
        let WsReply::Error(frame) = WsReply::from_handler_error("charge", carried) else {
            panic!("an error answers an error frame");
        };
        assert_eq!(frame.error, OPAQUE_CLIENT_MESSAGE);
        let lines = failed();
        assert_eq!(lines.len(), 1, "{lines:#?}");
        assert_eq!(lines[0].level, "error");
        assert!(
            lines[0]
                .field("error")
                .is_some_and(|e| e.contains("charging the card")),
            "{lines:#?}"
        );

        for (problem, said) in [
            (
                Problem::new(503, code::UNAVAILABLE),
                nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE,
            ),
            (Problem::new(409, code::CONFLICT), "conflict"),
        ] {
            let WsReply::Error(frame) = WsReply::from_handler_error("charge", problem) else {
                panic!("an error answers an error frame");
            };
            assert_eq!(frame.error, said);
        }
        assert_eq!(
            failed().len(),
            1,
            "a bare problem or a client one withholds nothing"
        );
        assert_eq!(answered().len(), 1, "{:#?}", answered());
    }

    /// A refusal a handler answers — its own `WsError`, a `4xx` problem, a
    /// pipe's rejection, a decode failure, bare or carried in a chain — is
    /// the client's answer: the message's line files it, and the edge files
    /// nothing beside it, as HTTP files its `4xx` on its access line alone.
    #[test]
    fn a_refusal_a_handler_answers_files_nothing_beside_its_line() {
        use nest_rs_core::anyhow::Error as Anyhow;
        use nest_rs_core::code;

        let logs = nest_rs_testing::LogCapture::install();
        let undecodable =
            || serde_json::from_str::<u64>(r#""sk_live_secret""#).expect_err("not a number");
        let frames = [
            WsReply::from_handler_error("buy", WsError::new("no room")),
            WsReply::from_handler_error("buy", Problem::new(409, code::CONFLICT)),
            WsReply::from_handler_error(
                "buy",
                Anyhow::from(Problem::new(409, code::CONFLICT)).context("reserving sku-42"),
            ),
            WsReply::from_handler_error("buy", PipeError::new("not for sale")),
            WsReply::from_handler_error("buy", undecodable()),
            WsReply::from_handler_error(
                "buy",
                Anyhow::from(undecodable()).context("reading the basket"),
            ),
            ErrorReport::new(Problem::new(422, code::INVALID_ARGUMENT))
                .into_failure()
                .into_reply("buy"),
        ];
        for frame in frames {
            let WsReply::Error(frame) = frame else {
                panic!("an error answers an error frame");
            };
            assert_ne!(frame.error, OPAQUE_CLIENT_MESSAGE, "{frame:?}");
        }
        assert!(
            logs.events()
                .iter()
                .all(|event| event.target != crate::TARGET || event.level == "debug"),
            "a client's refusal is no denial and no failure: {:#?}",
            logs.events(),
        );
    }

    /// A refusal holding an `Rc`, so it cannot cross threads, whose sentence
    /// names no cause.
    #[derive(Debug)]
    struct LocalRefusal {
        cause: std::rc::Rc<WsError>,
    }

    impl std::fmt::Display for LocalRefusal {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("reserving sku-42")
        }
    }

    impl Error for LocalRefusal {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&*self.cause)
        }
    }

    /// A refusal that cannot cross threads answers the frame its chain names,
    /// and the exception filters read its whole chain, not that frame's text.
    #[test]
    fn a_refusal_that_cannot_cross_threads_hands_the_filters_its_chain() {
        let report = ErrorReport::new(LocalRefusal {
            cause: std::rc::Rc::new(WsError::new("no room")),
        });

        let (rendered, failure) = ErrorReportLocal::into_failure(&report).into_parts();

        assert_eq!(rendered.frame.error, "no room");
        assert_eq!(failure.error().to_string(), "reserving sku-42: no room");
    }

    /// A non-string map key is the shape `serde_json` refuses.
    #[test]
    fn a_reply_that_cannot_be_serialized_tells_the_operator_and_not_the_client() {
        let logs = nest_rs_testing::LogCapture::install();
        let unserializable: std::collections::HashMap<(i32, i32), &str> =
            std::collections::HashMap::from([((1, 2), "corner")]);

        match WsReply::reply(&unserializable) {
            WsReply::Error(err) => crate::testing::assert_opaque(&err),
            _ => panic!("a value that cannot serialize must not read as a reply"),
        }

        let event = logs.expect_one("nest_rs::ws", "websocket message failed");
        assert_eq!(event.level, "error");
        assert!(
            event.field("error").is_some_and(|e| e.contains("key")),
            "the cause the frame withholds is what the log has to carry, got {:?}",
            event.fields,
        );
    }
}

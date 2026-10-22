use std::cell::Cell;
use std::error::Error;
use std::fmt;

use nest_rs_core::__private::{Withheld, find_in_chain};
use nest_rs_core::problem::code;
use nest_rs_core::{DecodeError, Failure, OPAQUE_CLIENT_MESSAGE, Problem, ToProblem};
use nest_rs_pipes::PipeError;
use serde::{Deserialize, Serialize};

use crate::opaque::Opaque;

/// What an event no handler serves is answered.
const UNKNOWN_EVENT: &str = "unknown event";

/// `{ "event": ..., "data": ... }` — the wire shape every gateway message
/// rides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsEnvelope {
    /// The message's event name, routing it and labelling its reply.
    pub event: String,
    /// The handler payload. Defaults to `null` when the frame omits `data`, so
    /// an argument-less event decodes without an explicit field.
    #[serde(default)]
    pub data: serde_json::Value,
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

/// What an error frame carries: the human-readable message, plus the structured
/// per-field detail when the failure had any, under HTTP's RFC 9457 `errors` name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsError {
    /// The human-readable message — the frame's `data.error`.
    pub error: String,
    /// Structured detail, usually per-field validation errors — the frame's
    /// `data.errors`, omitted when there is none (as HTTP omits it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errors: Option<serde_json::Value>,
}

impl WsError {
    /// A message with no structured detail.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
            errors: None,
        }
    }

    /// A message plus the structured detail a client can act on per field.
    pub fn with_details(message: impl Into<String>, details: serde_json::Value) -> Self {
        Self {
            error: message.into(),
            errors: Some(details),
        }
    }

    /// The frame a [`Problem`] answers: its
    /// [`client_message`](Problem::client_message), and its code — with its
    /// wait as `retryAfterSeconds` when it has one — under `errors`.
    ///
    /// ```
    /// use nest_rs_core::Problem;
    /// use nest_rs_core::problem::code;
    /// use nest_rs_ws::WsError;
    ///
    /// let frame = WsError::from_problem(&Problem::new(409, code::CONFLICT));
    /// assert_eq!(frame.error, "conflict");
    /// assert_eq!(frame.errors, Some(serde_json::json!({ "code": "CONFLICT" })));
    /// ```
    pub fn from_problem(problem: &Problem) -> Self {
        let mut errors = serde_json::Map::new();
        errors.insert("code".to_owned(), problem.code().as_str().into());
        if let Some(seconds) = problem.retry_after() {
            errors.insert("retryAfterSeconds".to_owned(), seconds.into());
        }
        Self::with_details(problem.client_message(), serde_json::Value::Object(errors))
    }

    /// The frame every failure the client is not told of answers, whoever
    /// answered it: the `INTERNAL` class's, [`OPAQUE_CLIENT_MESSAGE`] under its
    /// code.
    pub(crate) fn internal() -> Self {
        Self::from_problem(&Problem::new(500, code::INTERNAL))
    }

    /// The frame a [`PipeError`] answers: its message, and its field-level
    /// detail under `errors` when it has some.
    fn from_pipe(rejection: &PipeError) -> Self {
        Self {
            error: rejection.message().to_owned(),
            errors: rejection.details().cloned(),
        }
    }
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
pub enum WsReply {
    /// Send this value back on the request's event name.
    Reply(serde_json::Value),
    /// Send nothing — the handler returned `()` or chose to stay silent.
    None,
    /// Send an error frame `{ event, data: { error, errors? } }` and log a `warn`.
    Error(WsError),
}

impl WsReply {
    /// Serializes a handler's return; a failure degrades to an [`Opaque`]
    /// [`WsReply::Error`], since serde's `Display` can carry the value.
    pub fn reply<T: Serialize>(value: &T) -> WsReply {
        match serde_json::to_value(value).opaque() {
            Ok(data) => WsReply::Reply(data),
            Err(err) => WsReply::Error(err),
        }
    }

    /// Build an [`Error`](WsReply::Error) reply from an arbitrary message.
    pub fn error(message: impl Into<String>) -> WsReply {
        WsReply::Error(WsError::new(message))
    }

    /// The error frame a rejected [`PipeError`] puts on
    /// the wire, carrying its per-field detail, with a `warn` on `nest_rs::ws`.
    pub fn pipe_error(event: &str, what: &str, error: nest_rs_pipes::PipeError) -> WsReply {
        let message = format!("invalid {what} for `{event}`: {}", error.message());
        tracing::warn!(
            target: crate::TARGET,
            event,
            kind = what,
            error = error.message(),
            "subscribe_message rejected by a pipe",
        );
        match error.into_details() {
            Some(details) => WsReply::Error(WsError::with_details(message, details)),
            None => WsReply::Error(WsError::new(message)),
        }
    }

    /// The error frame a payload that does not deserialize puts on the wire,
    /// with a `warn`. Both say where it failed and what kind of value was found
    /// ([`DecodeError`]), never the value.
    pub fn payload_error(event: &str, error: &serde_json::Error) -> WsReply {
        let error = nest_rs_core::DecodeError::new(error);
        tracing::warn!(
            target: crate::TARGET,
            event,
            error = %error,
            "subscribe_message payload failed to deserialize",
        );
        WsReply::Error(WsError::new(format!(
            "invalid payload for `{event}`: {error}"
        )))
    }

    /// The reply for an event with no registered handler, so a client can tell
    /// a typo from an auth failure; the frame answers on the event it sent, and
    /// neither its text nor the `warn` repeats it, the event being only the
    /// client's word.
    pub fn unknown() -> WsReply {
        tracing::warn!(
            target: crate::TARGET,
            event = nest_rs_core::operation_log::OTHER,
            "subscribe_message dispatched to an unknown event",
        );
        WsReply::Error(WsError::new(UNKNOWN_EVENT))
    }

    /// The error frame a handler's `Err` produces, read off the vocabulary its
    /// chain carries: a [`WsError`] is sent whole, a [`Problem`] in its frame
    /// ([`WsError::from_problem`]), a [`PipeError`] with its detail, a decode
    /// failure without its value — each with a `warn` carrying the whole chain.
    /// Anything else answers [`OPAQUE_CLIENT_MESSAGE`] under the `INTERNAL`
    /// code, its chain filed once at `error`, as is the chain behind a `5xx`
    /// problem.
    pub fn from_handler_error<E>(event: &str, error: E) -> WsReply
    where
        E: Into<Box<dyn Error + Send + Sync>> + 'static,
    {
        chain_rendered(&*nest_rs_core::boxed_error(error)).into_reply(event)
    }
}

/// An error frame, and the line it files for the operator once the unit's
/// answer is final: an answer an exception filter replaced files nothing of
/// what it replaced.
#[derive(Debug)]
pub(crate) struct Rendered {
    /// What the client reads.
    pub(crate) frame: WsError,
    /// What the operator reads once the frame goes out.
    pub(crate) line: Option<Line>,
}

impl Rendered {
    /// The frame, its line filed at once for the message `event` declares.
    fn into_reply(self, event: &str) -> WsReply {
        if let Some(line) = self.line {
            line.file(event);
        }
        WsReply::Error(self.frame)
    }
}

/// The operator's line an error frame files once it goes out.
#[derive(Clone, Debug)]
pub(crate) enum Line {
    /// A refusal answered — or a `503`, the server unable to serve it in time
    /// — at `warn`, its whole chain.
    Answered(String),
    /// A failure the frame withholds, at `error`, its whole chain.
    Failed(String),
}

impl Line {
    /// File the line, for the message `event` declares.
    pub(crate) fn file(self, event: &str) {
        match self {
            Self::Answered(said) => tracing::warn!(
                target: crate::TARGET,
                event,
                error = %said,
                "subscribe_message handler returned Err",
            ),
            Self::Failed(said) => tracing::error!(
                target: crate::TARGET,
                event,
                error = %said,
                "websocket message failed",
            ),
        }
    }
}

/// The frame a failure's chain carries, else the opaque one.
pub(crate) fn chain_rendered(error: &(dyn Error + 'static)) -> Rendered {
    let frame = if let Some(frame) = find_in_chain::<WsError>(error) {
        frame.clone()
    } else if let Some(problem) = find_in_chain::<Problem>(error) {
        return problem_rendered(error, problem);
    } else if let Some(rejection) = find_in_chain::<PipeError>(error) {
        WsError::from_pipe(rejection)
    } else if let Some(report) = DecodeError::in_chain(error) {
        WsError::new(report.to_string())
    } else {
        return opaque_rendered(nest_rs_core::error_message(error));
    };
    Rendered {
        frame,
        line: Some(Line::Answered(nest_rs_core::error_message(error))),
    }
}

/// `problem`'s frame as the answer to `error`: a refusal answered — or a `503`,
/// the server unable to serve it in time — at `warn`, unless the `5xx` answer
/// withholds the chain behind a failure, which is then filed as an opaque
/// failure's is.
pub(crate) fn problem_rendered(error: &(dyn Error + 'static), problem: &Problem) -> Rendered {
    let line = match nest_rs_core::__private::withheld(problem, error) {
        Some(Withheld::Failed(said)) => Line::Failed(said),
        Some(Withheld::Unavailable(_)) | None => Line::Answered(nest_rs_core::error_message(error)),
    };
    Rendered {
        frame: WsError::from_problem(problem),
        line: Some(line),
    }
}

/// The problem an exception filter answered `error` with: only the chain a
/// `5xx` withholds is filed, so a failure a filter answers as a refusal files
/// nothing.
pub(crate) fn caught_rendered(error: &(dyn Error + 'static), problem: &Problem) -> Rendered {
    let line = match nest_rs_core::__private::withheld(problem, error) {
        Some(Withheld::Failed(said)) => Some(Line::Failed(said)),
        Some(Withheld::Unavailable(said)) => Some(Line::Answered(said)),
        None => None,
    };
    Rendered {
        frame: WsError::from_problem(problem),
        line,
    }
}

/// The opaque frame, with what the error said on the operator's line at `error`.
fn opaque_rendered(said: String) -> Rendered {
    Rendered {
        frame: WsError::internal(),
        line: Some(Line::Failed(said)),
    }
}

/// A handler's error, rendered by its type: the frame it answers, the line that
/// frame files once it goes out, and the failure the exception filters read —
/// the error itself, whenever it can cross threads.
pub struct HandlerFailed {
    rendered: Rendered,
    failure: Failure,
}

impl HandlerFailed {
    /// `rendered`, failing the unit with `failure`.
    fn new(rendered: Rendered, failure: Failure) -> Self {
        Self { rendered, failure }
    }

    /// The frame, its line filed at once for the message `event` declares:
    /// what a handler answers called on its own.
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
/// threads: its chain, as the operator's line says it.
#[derive(Debug)]
struct Unsendable(String);

impl fmt::Display for Unsendable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for Unsendable {}

/// The failure a handler's error that cannot cross threads leaves the
/// exception filters: its chain as `rendered` files it.
fn unsendable(rendered: Rendered) -> HandlerFailed {
    let said = match &rendered.line {
        Some(Line::Answered(said) | Line::Failed(said)) => said.clone(),
        None => rendered.frame.error.clone(),
    };
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

/// A handler's error, turned into the frame a client reads, the line an operator
/// reads once that frame goes out, and the failure the exception filters read —
/// **by type**, resolved where the expansion names it, in four tiers, each taken
/// only when the one above does not apply:
///
/// 1. A [`ToProblem`] error takes the inherent method: the problem it says, in
///    its frame ([`WsError::from_problem`]); `None` answers opaquely, its chain
///    filed at `error`, as is the chain behind a `5xx` problem.
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
        unsendable(chain_rendered(&*boxed(error)))
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
        unsendable(opaque_rendered(said))
    }
}

/// What a report answers once a tier took its error: unreachable, since each
/// call takes the report whole.
fn taken() -> HandlerFailed {
    unsendable(opaque_rendered(
        "a handler's error was rendered twice".to_owned(),
    ))
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
    /// repeats none of it.
    #[test]
    fn unknown_event_message_names_no_event() {
        match WsReply::unknown() {
            WsReply::Error(err) => assert_eq!(err.error, UNKNOWN_EVENT),
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

    #[test]
    fn pipe_error_carries_the_field_level_detail() {
        let details = serde_json::json!({ "text": [{ "code": "length" }] });
        let err = nest_rs_pipes::PipeError::with_details("validation failed", details);
        match WsReply::pipe_error("validated", "payload", err) {
            WsReply::Error(err) => {
                assert_eq!(
                    err.error,
                    "invalid payload for `validated`: validation failed"
                );
                assert_eq!(
                    err.errors.as_ref().and_then(|e| e.get("text")),
                    Some(&serde_json::json!([{ "code": "length" }])),
                    "the per-field errors reach the frame: {err:?}",
                );
            }
            _ => panic!("expected Error"),
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
            Some(Problem::new(503, nest_rs_core::problem::code::UNAVAILABLE).with_retry_after(30))
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
        use nest_rs_core::problem::code;

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
    /// files the line an opaque failure files, at `error`, by either tier; a
    /// bare problem or a client one is a refusal answered, at `warn`.
    #[test]
    fn a_server_problem_files_the_chain_its_frame_withholds() {
        use nest_rs_core::problem::code;

        let logs = nest_rs_testing::LogCapture::install();
        let failed = || logs.find(crate::TARGET, "websocket message failed");
        let answered = || logs.find(crate::TARGET, "subscribe_message handler returned Err");

        let WsReply::Error(frame) = ErrorReport::new(Outage).into_failure().into_reply("search")
        else {
            panic!("an error answers an error frame");
        };
        assert_eq!(frame.error, nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        assert_eq!(
            frame.errors,
            Some(serde_json::json!({ "code": "UNAVAILABLE", "retryAfterSeconds": 30 })),
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
        let refused = answered();
        assert_eq!(refused.len(), 3, "{refused:#?}");
        assert!(
            refused.iter().all(|line| line.level == "warn"),
            "{refused:#?}"
        );
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

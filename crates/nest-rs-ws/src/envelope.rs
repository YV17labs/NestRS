use serde::{Deserialize, Serialize};

use crate::opaque::Opaque;

/// `{ "event": ..., "data": ... }` — the wire shape every gateway message
/// rides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsEnvelope {
    /// The message's event name — how the dispatcher routes to a handler and
    /// how a reply is labelled on the wire.
    pub event: String,
    /// The handler payload. Defaults to `null` when the frame omits `data`, so
    /// an argument-less event decodes without an explicit field.
    #[serde(default)]
    pub data: serde_json::Value,
}

impl WsEnvelope {
    /// Serialize `data` into the `{ event, data }` wire frame for a given event
    /// name.
    ///
    /// Writes the payload straight into the frame: going through an owned
    /// `WsEnvelope` first would allocate the event `String` and a whole
    /// intermediate `Value` per frame — and for a broadcast, whose payload is
    /// *already* a `Value`, that intermediate was a full deep clone.
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
/// per-field detail when the failure had any.
///
/// A `Valid<T>` rejection attaches field-level errors to its `PipeError`, and the
/// socket used to drop them: a client got `"validation failed"` and no way to
/// know *which* field was wrong, while the same rejection over HTTP rendered an
/// RFC 9457 `errors` member. The frame now carries both members under the same
/// name HTTP uses, so one payload shape describes a rejection on either
/// transport.
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
}

impl std::fmt::Display for WsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error)
    }
}

/// A handler may return it as its `Err`: it is the frame a client reads, so
/// [`WsReply::from_handler_error`] sends it whole, details included — when it is
/// the handler's error type itself; wrapped in `anyhow`, only its sentence
/// survives the conversion.
impl std::error::Error for WsError {}

impl<T: Into<String>> From<T> for WsError {
    fn from(message: T) -> Self {
        Self::new(message)
    }
}

/// Dispatch outcome the connection loop turns into a frame (or silence).
pub enum WsReply {
    /// Send this value back on the request's event name.
    Reply(serde_json::Value),
    /// Send nothing — the handler returned `()` or chose to stay silent.
    None,
    /// Send an error frame `{ event, data: { error, errors? } }` and log a `warn`.
    Error(WsError),
}

impl WsReply {
    /// Serializes a handler's return; a failure degrades to [`WsReply::Error`].
    ///
    /// Through [`Opaque`](crate::Opaque), like every other failure on this edge,
    /// and for both of its reasons at once. It said nothing at all — the one
    /// silent site in this file, next door to [`pipe_error`](Self::pipe_error),
    /// whose own doc records learning the same lesson — and what it handed the
    /// socket was `serde_json::Error`'s `Display`, which names the handler's
    /// types and can carry the value that would not serialize. A reply that
    /// cannot be built is the *server's* failure, so the operator gets the cause
    /// and the client gets the constant.
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

    /// The error frame a rejected [`PipeError`](nest_rs_pipes::PipeError) puts on
    /// the wire, carrying its per-field detail. The single site both the payload
    /// pipe and the global data pipe route through, so a client sees the same
    /// shape whichever one rejected.
    ///
    /// Warns on `nest_rs::ws`, like every other refused dispatch. It used to be
    /// silent, which was exactly backwards: a *client* sending an invalid
    /// payload is the case an operator wants in the log, and the server's own
    /// deliberate `Err` was the only one that appeared.
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

    /// The error frame a payload that does not deserialize puts on the wire.
    ///
    /// Its own constructor rather than a bare [`error`](Self::error) so the
    /// `warn` cannot be forgotten at the one call site that produces it —
    /// malformed input from a client is a denied dispatch like any other.
    pub fn payload_error(event: &str, error: &(dyn std::error::Error + 'static)) -> WsReply {
        tracing::warn!(
            target: crate::TARGET,
            event,
            error = %nest_rs_core::error_message(error),
            "subscribe_message payload failed to deserialize",
        );
        WsReply::Error(WsError::new(format!(
            "invalid payload for `{event}`: {error}"
        )))
    }

    /// The reply for an event with no registered handler — names the offending
    /// event so a client can tell a typo from an auth failure.
    pub fn unknown(event: &str) -> WsReply {
        tracing::warn!(
            target: crate::TARGET,
            event,
            "subscribe_message dispatched to an unknown event",
        );
        WsReply::Error(WsError::new(format!("unknown event `{event}`")))
    }

    /// The error frame a handler's `Err` produces, with the `warn` that makes
    /// it greppable. One implementation for both routes into it — the
    /// `#[subscribe_message]` expansion's syntactic `Result` arm and the
    /// type-directed [`ReplyValue`] fallback — so the two can never disagree
    /// on what a failed handler puts on the wire.
    ///
    /// The operator's line carries the whole cause chain; the client's frame
    /// carries the error's own sentence, as it always has. Any error converting
    /// into a boxed one qualifies — `anyhow::Error` keeps its context chain
    /// through that conversion.
    pub fn from_handler_error<E>(event: &str, error: E) -> WsReply
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let error: Box<dyn std::error::Error + Send + Sync> = error.into();
        tracing::warn!(
            target: crate::TARGET,
            event,
            error = %nest_rs_core::error_message(&*error),
            "subscribe_message handler returned Err",
        );
        match error.downcast::<WsError>() {
            Ok(frame) => WsReply::Error(*frame),
            Err(error) => WsReply::Error(WsError::new(error.to_string())),
        }
    }
}

/// Turns a handler's return value into a [`WsReply`] **by type**, closing the
/// gap the macro's syntactic detection leaves.
///
/// `#[subscribe_message]` reads the return type's last path segment to decide
/// whether a handler can fail. That works for `Result<T, E>` and for
/// `anyhow::Result<T>`, and silently does not for an alias —
/// `pub type ServiceResult<T> = Result<T, MyError>` reads as an ordinary value,
/// so the `Err` variant was serialized straight into the reply `data`: the
/// whole error struct, every field, under a frame shaped like a success (no
/// `error` key), with no `warn` server-side because nothing knew a failure had
/// happened. It compiled without a warning, and only in codebases with typed
/// `Serialize` errors — the ones whose errors carry the most detail.
///
/// Resolution here is on the type, so an alias is transparent: the inherent
/// method below applies to **any** `Result<T, E>` however it is spelled — no
/// bound on `E`, because a bound it failed would drop an `Err` onto the blanket
/// method and serialize it whole — and wins over the blanket trait method
/// (inherent methods are probed first). What becomes of the `Err` is
/// [`ErrorReport`]'s to decide, where the expansion knows its concrete type.
///
/// **A handler returns at most a `Result` around a `Result`-free value.** The
/// expansion splits the value twice — the outer `Result`, then its `Ok` — so
/// `Result<Result<T, E1>, E2>` is a failure on either `Err`, literal or aliased.
/// It used to split once, and the inner `Err` shipped as `{"Err": …}` in a frame
/// shaped like a success. A `Result` any deeper, or inside an `Option`, a `Vec`
/// or a struct, is data and is serialized as such.
pub struct ReplyValue<T>(pub T);

/// A handler's return value, split into what replies and what failed.
pub enum ReplyOutcome<T, E> {
    /// The value a success replies with — split again, or serialized.
    Value(T),
    /// The error an `Err` carried, for [`ErrorReport`] to turn into a frame.
    Failed(E),
}

impl<T, E> ReplyValue<Result<T, E>> {
    /// A `Result` however it was spelled: `Ok` is the value, `Err` is handed back.
    pub fn into_outcome(self) -> ReplyOutcome<T, E> {
        match self.0 {
            Ok(value) => ReplyOutcome::Value(value),
            Err(err) => ReplyOutcome::Failed(err),
        }
    }
}

/// The ordinary case: any other value is the value. Kept a trait so the
/// `Result` impl above can be inherent, and therefore more specific.
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

/// A handler's error, turned into the frame a client reads and the line an
/// operator reads — **by type**, resolved where the expansion names it, in three
/// tiers, each taken only when the one above does not apply:
///
/// 1. An error converting into a boxed `Send + Sync` one takes the inherent
///    method: its whole cause chain is logged, and a [`WsError`] is sent whole
///    ([`WsReply::from_handler_error`]).
/// 2. Any other error — one holding an `Rc`, a `Box<dyn Error>` — takes
///    [`ErrorReportChain`]: it cannot cross threads, but it has causes, and they
///    are logged. It used to fall to the third tier and lose them.
/// 3. Any other `Display` type takes [`ErrorReportFallback`]: it has no causes to
///    walk, so its sentence is its whole chain.
///
/// A type that is none of them does not compile.
pub struct ErrorReport<E>(pub E);

impl<E> ErrorReport<E>
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    /// The error frame, with the whole cause chain on the operator's line.
    pub fn into_frame(self, event: &str) -> WsReply {
        WsReply::from_handler_error(event, self.0)
    }
}

/// The error that is not `Send + Sync` case of [`ErrorReport`] — the second tier.
pub trait ErrorReportChain {
    /// The error frame, with the whole cause chain on the operator's line.
    fn into_frame(self, event: &str) -> WsReply;
}

impl<E> ErrorReportChain for ErrorReport<E>
where
    E: Into<Box<dyn std::error::Error>>,
{
    fn into_frame(self, event: &str) -> WsReply {
        let error: Box<dyn std::error::Error> = self.0.into();
        tracing::warn!(
            target: crate::TARGET,
            event,
            error = %nest_rs_core::error_message(&*error),
            "subscribe_message handler returned Err",
        );
        WsReply::Error(WsError::new(error.to_string()))
    }
}

/// The `Display`-only case of [`ErrorReport`] — the third tier, implemented on a
/// borrow so that method resolution reaches it only after the two above.
pub trait ErrorReportFallback {
    /// The error frame, with the error's sentence on the operator's line.
    fn into_frame(self, event: &str) -> WsReply;
}

impl<E: std::fmt::Display> ErrorReportFallback for &ErrorReport<E> {
    fn into_frame(self, event: &str) -> WsReply {
        tracing::warn!(
            target: crate::TARGET,
            event,
            error = %self.0,
            "subscribe_message handler returned Err",
        );
        WsReply::Error(WsError::new(self.0.to_string()))
    }
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

    #[test]
    fn unknown_event_message_names_the_event() {
        match WsReply::unknown("chat:nope") {
            WsReply::Error(err) => assert!(err.error.contains("chat:nope")),
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

    // The finding: a `Valid<T>` rejection attached per-field errors to its
    // `PipeError` and the socket formatted only `message()`, so a client learned
    // that validation failed but not which field.
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

    /// A reply that cannot be serialized is the server failing, and it used to
    /// fail *silently* while handing the socket serde's own `Display`.
    ///
    /// Both halves are asserted here because either one alone passes for the
    /// wrong reason: a constant on the wire proves nothing if the cause went
    /// nowhere, and an event proves nothing if the frame still carries the
    /// types. The map keys are the shape `serde_json` refuses — a non-string
    /// key — so nothing here depends on a hand-written failing `Serialize`.
    #[test]
    fn a_reply_that_cannot_be_serialized_tells_the_operator_and_not_the_client() {
        let logs = nest_rs_testing::LogCapture::install();
        let unserializable: std::collections::HashMap<(i32, i32), &str> =
            std::collections::HashMap::from([((1, 2), "corner")]);

        match WsReply::reply(&unserializable) {
            WsReply::Error(err) => {
                assert_eq!(err.error, nest_rs_core::OPAQUE_CLIENT_MESSAGE);
                assert!(
                    err.errors.is_none(),
                    "an opaque failure offers no per-field detail: {err:?}",
                );
            }
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

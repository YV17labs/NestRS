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
            target: "nest_rs::ws",
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
    pub fn payload_error(event: &str, error: &impl std::fmt::Display) -> WsReply {
        tracing::warn!(
            target: "nest_rs::ws",
            event,
            error = %error,
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
            target: "nest_rs::ws",
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
    pub fn from_handler_error(event: &str, error: &impl std::fmt::Display) -> WsReply {
        tracing::warn!(
            target: "nest_rs::ws",
            event,
            error = %error,
            "subscribe_message handler returned Err",
        );
        WsReply::Error(WsError::new(error.to_string()))
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
/// method below applies to any `Result<T, E>` however it is spelled, and wins
/// over the blanket trait method (inherent methods are probed first).
pub struct ReplyValue<'a, T>(pub &'a T);

impl<T: Serialize, E: std::fmt::Display> ReplyValue<'_, Result<T, E>> {
    /// A `Result` however it was spelled: `Ok` replies, `Err` becomes the same
    /// error frame the literal form produces.
    pub fn into_reply(self, event: &str) -> WsReply {
        match self.0 {
            Ok(value) => WsReply::reply(value),
            Err(err) => WsReply::from_handler_error(event, err),
        }
    }
}

/// The ordinary case: any serializable value replies as-is. Kept a trait so the
/// `Result` impl above can be inherent, and therefore more specific.
pub trait ReplyValueFallback {
    /// Serialize the value into a reply on the request's event name.
    fn into_reply(self, event: &str) -> WsReply;
}

impl<T: Serialize> ReplyValueFallback for ReplyValue<'_, T> {
    fn into_reply(self, _event: &str) -> WsReply {
        WsReply::reply(self.0)
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

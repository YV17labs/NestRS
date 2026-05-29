use serde::{Deserialize, Serialize};

/// The wire format every gateway message rides in: a named event plus an opaque
/// JSON `data` payload — the shape NestJS's `@SubscribeMessage` mapping uses.
/// `#[messages]` deserializes `data` into a handler's payload type and serializes
/// the handler's return back into a `data` under the same event name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsEnvelope {
    pub event: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl WsEnvelope {
    /// Render an outgoing message — `{ "event": ..., "data": ... }` — from an
    /// event name and any serializable payload. The single place the wire shape
    /// is built, shared by the registry's server→client pushes and the gateway's
    /// error frames.
    pub fn encode<T: Serialize>(event: &str, data: &T) -> Result<String, serde_json::Error> {
        serde_json::to_string(&WsEnvelope {
            event: event.to_string(),
            data: serde_json::to_value(data)?,
        })
    }
}

/// The outcome of dispatching one incoming message — what `Gateway::dispatch`
/// returns and the connection loop turns into a reply frame (or silence).
pub enum WsReply {
    /// Serialized handler return; sent back under the request's event name.
    Reply(serde_json::Value),
    /// The handler returned `()` — send nothing.
    None,
    /// A parse/dispatch failure; sent back as `data: { "error": msg }`.
    Error(String),
}

impl WsReply {
    /// Serialize a handler's return value into a reply. A serialization failure
    /// degrades to an [`WsReply::Error`] rather than dropping the message.
    pub fn reply<T: Serialize>(value: &T) -> WsReply {
        match serde_json::to_value(value) {
            Ok(data) => WsReply::Reply(data),
            Err(err) => WsReply::Error(format!("failed to serialize reply: {err}")),
        }
    }

    /// A dispatch/parse error to relay to the client.
    pub fn error(message: impl Into<String>) -> WsReply {
        WsReply::Error(message.into())
    }

    /// No `#[subscribe_message]` handler matched the event.
    pub fn unknown(event: &str) -> WsReply {
        WsReply::Error(format!("unknown event `{event}`"))
    }
}

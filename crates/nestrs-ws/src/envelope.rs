use serde::{Deserialize, Serialize};

/// `{ "event": ..., "data": ... }` — the wire shape every gateway message
/// rides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsEnvelope {
    pub event: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl WsEnvelope {
    pub fn encode<T: Serialize>(event: &str, data: &T) -> Result<String, serde_json::Error> {
        serde_json::to_string(&WsEnvelope {
            event: event.to_string(),
            data: serde_json::to_value(data)?,
        })
    }
}

/// Dispatch outcome the connection loop turns into a frame (or silence).
pub enum WsReply {
    Reply(serde_json::Value),
    None,
    Error(String),
}

impl WsReply {
    /// Serializes a handler's return; a failure degrades to [`WsReply::Error`].
    pub fn reply<T: Serialize>(value: &T) -> WsReply {
        match serde_json::to_value(value) {
            Ok(data) => WsReply::Reply(data),
            Err(err) => WsReply::Error(format!("failed to serialize reply: {err}")),
        }
    }

    pub fn error(message: impl Into<String>) -> WsReply {
        WsReply::Error(message.into())
    }

    pub fn unknown(event: &str) -> WsReply {
        WsReply::Error(format!("unknown event `{event}`"))
    }
}

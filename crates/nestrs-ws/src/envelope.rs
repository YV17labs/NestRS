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
            WsReply::Error(msg) => assert!(msg.contains("chat:nope")),
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn error_constructor_carries_the_message() {
        match WsReply::error("boom") {
            WsReply::Error(msg) => assert_eq!(msg, "boom"),
            _ => panic!("expected Error"),
        }
    }
}

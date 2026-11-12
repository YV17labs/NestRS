//! [`Pings`], what a mount's sessions are pinged on, and MCP's `ping` request
//! (the specification's *Ping* utility) as the mount asks it: [`ping`], and
//! [`is_ping_answer`] for what comes back.
//!
//! The ping is written on the stream a session's client listens on
//! ([`SessionStream`](crate::session_stream::SessionStream)), never sent
//! through rmcp's session worker, which would keep one sent while no stream is
//! open to replay later. Its answer is taken out before the worker sees it, and
//! neither is use of the session.

use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{
    ClientJsonRpcMessage, NumberOrString, PingRequest, ServerJsonRpcMessage, ServerRequest,
};
use rmcp::transport::streamable_http_server::session::ServerSseMessage;

/// How every id the mount's pings take opens: rmcp numbers its own requests,
/// so no request of rmcp's, or of a host writing through its peer, takes one.
/// A random UUID follows, since a session restored from the app's store, or
/// served by another instance, gets a client of its own and no count to go on.
const PING_ID_PREFIX: &str = "ping-";

/// What one mount's sessions are pinged on, and the mount their line names.
#[derive(Clone, Debug)]
pub(crate) struct Pings {
    interval: Option<Duration>,
    timeout: Duration,
    path: Option<Arc<str>>,
}

impl Pings {
    /// A ping once a session's client is silent for `interval`, its session
    /// ended when nothing comes back within `timeout`, on the mount served at
    /// `path`; `interval` `None` pings no one.
    pub(crate) fn new(
        interval: Option<Duration>,
        timeout: Duration,
        path: Option<Arc<str>>,
    ) -> Self {
        Self {
            interval,
            timeout,
            path,
        }
    }

    /// How long a client may be silent before it is asked; `None` asks no one.
    pub(crate) fn interval(&self) -> Option<Duration> {
        self.interval
    }

    /// How long a ping waits for anything back.
    pub(crate) fn timeout(&self) -> Duration {
        self.timeout
    }

    /// How long a client listening on a stream may be silent before its session
    /// ends: the interval, then the timeout of the ping it brings, whether or
    /// not the transport could write that ping. `None` while no one is asked.
    pub(crate) fn silence(&self) -> Option<Duration> {
        self.interval.map(|interval| interval + self.timeout)
    }

    /// The path of the mount these pings ask on, which its line names.
    pub(crate) fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
}

/// A ping, under an id no ping of its session took before, whichever client of
/// that session asks it.
pub(crate) fn ping() -> ServerSseMessage {
    let id = format!("{PING_ID_PREFIX}{}", uuid::Uuid::new_v4().simple());
    ServerSseMessage::from_message(ServerJsonRpcMessage::request(
        ServerRequest::PingRequest(PingRequest::default()),
        NumberOrString::String(Arc::from(id)),
    ))
}

/// Whether `message` answers one of the mount's pings — its result, or an
/// error from a client that refused it — which rmcp's worker never sees.
pub(crate) fn is_ping_answer(message: &ClientJsonRpcMessage) -> bool {
    let id = match message {
        ClientJsonRpcMessage::Response(response) => Some(&response.id),
        ClientJsonRpcMessage::Error(error) => error.id.as_ref(),
        _ => None,
    };
    matches!(id, Some(NumberOrString::String(id)) if id.starts_with(PING_ID_PREFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session restored from the app's store, or served by another instance,
    /// gets a client of its own, with no count to go on: its pings never take an
    /// id one asked before within the session.
    #[test]
    fn a_session_bound_again_never_asks_under_an_id_it_used() {
        let id = |ping: ServerSseMessage| {
            serde_json::to_value(ping.message.as_deref().expect("a message")).expect("serializes")
                ["id"]
                .clone()
        };

        let asked = id(ping());
        let asked_again = id(ping());

        assert_ne!(asked, asked_again, "one id, asked twice in one session");
        for asked in [asked, asked_again] {
            let answer: ClientJsonRpcMessage = serde_json::from_value(
                serde_json::json!({ "jsonrpc": "2.0", "id": asked, "result": {} }),
            )
            .expect("an answer");
            assert!(is_ping_answer(&answer), "{asked}");
        }
    }

    #[test]
    fn only_an_answer_to_a_ping_of_the_mounts_is_one() {
        let message = |value: serde_json::Value| -> ClientJsonRpcMessage {
            serde_json::from_value(value).expect("a message")
        };
        assert!(is_ping_answer(&message(
            serde_json::json!({ "jsonrpc": "2.0", "id": "ping-3", "result": {} })
        )));
        assert!(is_ping_answer(&message(serde_json::json!({
            "jsonrpc": "2.0",
            "id": "ping-3",
            "error": { "code": -32601, "message": "Method not found" },
        }))));
        for other in [
            serde_json::json!({ "jsonrpc": "2.0", "id": 3, "result": {} }),
            serde_json::json!({ "jsonrpc": "2.0", "id": "elicit-3", "result": {} }),
            serde_json::json!({ "jsonrpc": "2.0", "id": "ping-3", "method": "ping" }),
            serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        ] {
            assert!(!is_ping_answer(&message(other.clone())), "{other}");
        }
    }
}

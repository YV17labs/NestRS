//! Driving a GraphQL **subscription** over graphql-ws.
//!
//! A query is a `POST`, so [`TestApp`](crate::TestApp)'s client already speaks
//! it. A subscription is not: it rides a WebSocket, and the thing a suite has to
//! reproduce is the protocol on top of that socket — `connection_init` →
//! `connection_ack` → `subscribe` → `next`… → `complete`. Hand-rolling it per
//! suite means each copy re-encodes the message names and the ordering, and they
//! drift, so it lives here once.
//!
//! No socket is bound. The protocol engine
//! ([`async_graphql::http::WebSocket`]) is the same one the mount runs above
//! poem's upgrade — it takes an executor and a stream of client messages, which
//! is exactly what a test can supply. What is *not* exercised here is the
//! upgrade itself (the guard that authenticates it, the lifetime ceiling that
//! bounds it); that half needs a real socket and belongs in an app's e2e suite.
//!
//! ```ignore
//! let mut socket = app.graphql_socket().data(ability).open();
//! socket.connect().await;
//! socket.subscribe("1", "subscription { postPublished { id } }").await;
//! let item = socket.next_item("1").await.expect("an item");
//! ```

use std::pin::Pin;
use std::time::Duration;

use async_graphql::Data;
use async_graphql::futures_util::stream::{Stream, StreamExt};
use async_graphql::http::WsMessage;
use nest_rs_core::Container;
use nest_rs_graphql::GraphqlConfig;
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// How long [`GraphqlSocket::next_message`] waits before reporting silence.
/// Long enough that a loaded CI box does not flake, short enough that a test
/// asserting *absence* stays quick.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Builds a [`GraphqlSocket`] against an app's composed schema.
pub struct GraphqlSocketBuilder {
    container: Container,
    data: Data,
}

impl GraphqlSocketBuilder {
    pub(crate) fn new(container: Container) -> Self {
        Self {
            container,
            data: Data::default(),
        }
    }

    /// Attach a value to the connection's context — what the operation guard
    /// installs on a real upgrade (the caller's `Ability`, a principal). A
    /// subscription reads it exactly as it would there.
    #[must_use]
    pub fn data<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        self.data.insert(value);
        self
    }

    /// Open the connection. The schema is composed from the app's container and
    /// its resolved [`GraphqlConfig`], so depth/complexity limits and
    /// introspection match what the mount serves.
    pub fn open(self) -> GraphqlSocket {
        let config = self
            .container
            .get::<GraphqlConfig>()
            .map(|config| (*config).clone())
            .unwrap_or_default();
        let executor = nest_rs_graphql::compose_schema(self.container, &config);
        let (to_server, rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let client = tokio_stream_from(rx);
        let from_server = async_graphql::http::WebSocket::new(
            executor,
            client,
            async_graphql::http::WebSocketProtocols::GraphQLWS,
        )
        .connection_data(self.data);
        GraphqlSocket {
            to_server,
            from_server: Box::pin(from_server),
        }
    }
}

fn tokio_stream_from(
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
) -> impl Stream<Item = Vec<u8>> + Send + 'static {
    async_graphql::futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|message| (message, rx))
    })
}

/// One graphql-ws connection, driven message by message.
pub struct GraphqlSocket {
    to_server: mpsc::UnboundedSender<Vec<u8>>,
    from_server: Pin<Box<dyn Stream<Item = WsMessage> + Send>>,
}

impl GraphqlSocket {
    /// Send `connection_init` and await `connection_ack`. Every graphql-ws
    /// exchange starts here; a server that answers anything else is refusing the
    /// connection, so this panics rather than letting the next assertion fail
    /// somewhere less obvious.
    pub async fn connect(&mut self) {
        self.send(json!({ "type": "connection_init" }));
        let ack = self
            .next_message()
            .await
            .expect("the server answers connection_init");
        assert_eq!(
            ack["type"], "connection_ack",
            "the server refused the connection: {ack}",
        );
    }

    /// Start operation `id`.
    pub fn subscribe(&mut self, id: &str, query: &str) {
        self.send(json!({
            "id": id,
            "type": "subscribe",
            "payload": { "query": query },
        }));
    }

    /// Stop operation `id`.
    pub fn stop(&mut self, id: &str) {
        self.send(json!({ "id": id, "type": "complete" }));
    }

    /// The next server message, whatever its type. `None` on silence (after
    /// [`DEFAULT_TIMEOUT`]) or once the connection is closed.
    pub async fn next_message(&mut self) -> Option<Value> {
        self.next_message_within(DEFAULT_TIMEOUT).await
    }

    /// [`next_message`](Self::next_message) with an explicit budget — use a
    /// short one when asserting that **nothing** arrives, so the test does not
    /// pay the full timeout to prove silence.
    pub async fn next_message_within(&mut self, within: Duration) -> Option<Value> {
        let message = tokio::time::timeout(within, self.from_server.next())
            .await
            .ok()??;
        match message {
            WsMessage::Text(text) => {
                Some(serde_json::from_str(&text).expect("a graphql-ws message is JSON"))
            }
            // The server closed the connection: no further message will come, so
            // a caller waiting on one is told to stop rather than left to the
            // timeout.
            WsMessage::Close(..) => None,
        }
    }

    /// The next `next` payload for operation `id` — the shape a subscriber
    /// actually reads. `error` and `complete` for that id end the wait and are
    /// returned as-is, so a test never blocks on a stream the server has already
    /// finished.
    pub async fn next_item(&mut self, id: &str) -> Option<Value> {
        self.next_item_within(id, DEFAULT_TIMEOUT).await
    }

    /// [`next_item`](Self::next_item) with an explicit budget.
    pub async fn next_item_within(&mut self, id: &str, within: Duration) -> Option<Value> {
        while let Some(message) = self.next_message_within(within).await {
            if message["id"] != id {
                continue;
            }
            match message["type"].as_str() {
                Some("next") => return Some(message["payload"].clone()),
                Some("error") | Some("complete") => return Some(message),
                _ => continue,
            }
        }
        None
    }

    fn send(&mut self, message: Value) {
        let _ = self.to_server.send(message.to_string().into_bytes());
    }
}

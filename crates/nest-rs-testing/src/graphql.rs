//! Driving a GraphQL **subscription** over graphql-transport-ws.
//!
//! The driver speaks `graphql-transport-ws` alone (async-graphql's
//! `WebSocketProtocols::GraphQLWS`, which reads backwards), not the legacy
//! `subscriptions-transport-ws`.
//!
//! No socket is bound: the mount's own protocol engine
//! ([`async_graphql::http::WebSocket`]) runs over a channel. The upgrade itself
//! — its guard, its lifetime ceiling, the protocol negotiation,
//! `GraphqlConfig::max_connection` — needs a real socket and an app's e2e suite.
//!
//! ```
//! # use nest_rs_core::module;
//! # use nest_rs_graphql::async_graphql::{Context, SimpleObject};
//! # use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream};
//! # use nest_rs_graphql::{GraphqlModule, operations, resolver};
//! # use nest_rs_testing::TestApp;
//! #
//! # #[derive(SimpleObject)]
//! # struct Post {
//! #     id: i32,
//! # }
//! #
//! # struct Viewer(i32);
//! #
//! # #[resolver]
//! # struct PostsResolver;
//! #
//! # #[operations]
//! # impl PostsResolver {
//! #     #[query]
//! #     #[public]
//! #     async fn post_count(&self) -> i32 {
//! #         1
//! #     }
//! #
//! #     #[subscription]
//! #     #[public]
//! #     fn post_published(&self, ctx: &Context<'_>) -> impl Stream<Item = Post> {
//! #         stream::iter([Post { id: ctx.data_unchecked::<Viewer>().0 }])
//! #     }
//! # }
//! #
//! # #[module(imports = [GraphqlModule::for_root(None)], providers = [PostsResolver])]
//! # struct AppModule;
//! #
//! # #[nest_rs_core::main]
//! # async fn main() -> anyhow::Result<()> {
//! # let app = TestApp::for_module::<AppModule>().await?;
//! let mut socket = app.graphql_socket().data(Viewer(7)).open();
//! socket.connect().await;
//! socket.subscribe("1", "subscription { postPublished { id } }");
//! let item = socket.next_item("1").await.expect("an item");
//! assert_eq!(item["data"]["postPublished"]["id"], 7);
//! # Ok(())
//! # }
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

    /// Attach a value to the connection's context, as the operation guard does
    /// on a real upgrade (the caller's `Ability`, a principal).
    #[must_use]
    pub fn data<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        self.data.insert(value);
        self
    }

    /// Open the connection, over the schema composed from the app's container
    /// and its resolved [`GraphqlConfig`].
    pub fn open(self) -> GraphqlSocket {
        let config = self
            .container
            .get::<GraphqlConfig>()
            .map(|config| (*config).clone())
            .unwrap_or_default();
        let executor = nest_rs_graphql::__private::compose_schema(self.container, &config);
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

/// What one read off a graphql-transport-ws connection found.
///
/// Only [`Silent`](Self::Silent), never [`Ended`](Self::Ended), proves the
/// server chose to say nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphqlEvent {
    /// A protocol message.
    Message(Value),
    /// The server closed, with the protocol close code and its reason.
    Closed(u16, String),
    /// Nothing arrived within the budget, and the connection is still open.
    Silent,
    /// The message stream ended with no close frame — over a real socket,
    /// RFC 6455 §7.4.1's **1006 Abnormal Closure**.
    Ended,
}

/// Why a connection attempt did not reach `connection_ack`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphqlRefusal {
    /// The server closed instead of acknowledging — the code says why.
    Closed(u16, String),
    /// The server answered a message that was not `connection_ack`.
    Message(Value),
    /// The server said nothing at all, and the connection is still open.
    Silent,
    /// The engine dropped the connection without answering.
    Ended,
}

/// One graphql-transport-ws connection, driven message by message.
pub struct GraphqlSocket {
    to_server: mpsc::UnboundedSender<Vec<u8>>,
    from_server: Pin<Box<dyn Stream<Item = WsMessage> + Send>>,
}

impl GraphqlSocket {
    /// Send `connection_init` and await `connection_ack`, panicking on any
    /// other answer; [`try_connect`](Self::try_connect) asserts a refusal.
    pub async fn connect(&mut self) {
        if let Err(refusal) = self.try_connect().await {
            panic!("the server refused the connection: {refusal:?}");
        }
    }

    /// [`connect`](Self::connect), reporting a refused connection instead of
    /// panicking.
    ///
    /// graphql-transport-ws refuses with a close code: **4400** invalid message,
    /// **4401** unauthorized (an operation before the ack), **4403** forbidden,
    /// **4408** init timeout, **4409** duplicate subscriber id, **4429** too
    /// many init requests.
    pub async fn try_connect(&mut self) -> Result<(), GraphqlRefusal> {
        self.send(json!({ "type": "connection_init" }));
        match self.next_event().await {
            GraphqlEvent::Message(ack) if ack["type"] == "connection_ack" => Ok(()),
            GraphqlEvent::Message(other) => Err(GraphqlRefusal::Message(other)),
            GraphqlEvent::Closed(code, reason) => Err(GraphqlRefusal::Closed(code, reason)),
            GraphqlEvent::Silent => Err(GraphqlRefusal::Silent),
            GraphqlEvent::Ended => Err(GraphqlRefusal::Ended),
        }
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
    /// `DEFAULT_TIMEOUT`) or once the connection is closed.
    pub async fn next_message(&mut self) -> Option<Value> {
        self.next_message_within(DEFAULT_TIMEOUT).await
    }

    /// [`next_message`](Self::next_message) with an explicit budget.
    pub async fn next_message_within(&mut self, within: Duration) -> Option<Value> {
        match self.next_event_within(within).await {
            GraphqlEvent::Message(message) => Some(message),
            GraphqlEvent::Closed(..) | GraphqlEvent::Silent | GraphqlEvent::Ended => None,
        }
    }

    /// The next server event, keeping the close code
    /// [`next_message`](Self::next_message) discards.
    pub async fn next_event(&mut self) -> GraphqlEvent {
        self.next_event_within(DEFAULT_TIMEOUT).await
    }

    /// [`next_event`](Self::next_event) with an explicit budget.
    pub async fn next_event_within(&mut self, within: Duration) -> GraphqlEvent {
        let Ok(next) = tokio::time::timeout(within, self.from_server.next()).await else {
            return GraphqlEvent::Silent;
        };
        match next {
            Some(WsMessage::Text(text)) => GraphqlEvent::Message(
                serde_json::from_str(&text).expect("a graphql-transport-ws message is JSON"),
            ),
            Some(WsMessage::Close(code, reason)) => GraphqlEvent::Closed(code, reason),
            None => GraphqlEvent::Ended,
        }
    }

    /// The next `next` payload for operation `id`; an `error` or `complete` for
    /// that id ends the wait and is returned as-is.
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
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a send after the engine ended surfaces as GraphqlEvent::Ended on the next read"
        )]
        let _ = self.to_server.send(message.to_string().into_bytes());
    }
}

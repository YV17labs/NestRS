//! WebSocket gateways for nestrs.
//!
//! A `#[gateway]` struct with a `#[messages]` impl holds
//! `#[subscribe_message("event")]` handlers. Messages ride a JSON envelope
//! `{ "event": "...", "data": ... }`. A gateway self-mounts on the HTTP
//! transport — listing it in `#[module(providers = [...])]` is the entire
//! wiring; it inherits port, CORS and TLS.
//!
//! ```
//! use nest_rs_ws::{Gateway, WsClient, WsReply, gateway, messages};
//! # use nest_rs_core::{Layer, injectable};
//! # use nest_rs_guards::{Guard, HttpGuard};
//! # use nest_rs_ws::{async_trait, input};
//! #
//! # #[injectable]
//! # #[derive(Default)]
//! # struct AuthnGuard;
//! #
//! # impl Layer for AuthnGuard {}
//! #
//! # #[async_trait]
//! # impl Guard for AuthnGuard {}
//! #
//! # impl HttpGuard for AuthnGuard {}
//! #
//! # #[input]
//! # struct SendMessage {
//! #     text: String,
//! # }
//! #
//! # #[derive(serde::Serialize)]
//! # struct ChatMessage {
//! #     text: String,
//! # }
//!
//! #[gateway(path = "/ws")]
//! #[use_guards(AuthnGuard)]
//! struct ChatGateway;
//!
//! #[messages]
//! impl ChatGateway {
//!     #[subscribe_message("message")]
//!     #[public]
//!     async fn on_message(&self, msg: SendMessage) -> ChatMessage {
//!         ChatMessage { text: msg.text }
//!     }
//! }
//! # #[nest_rs_core::main]
//! # async fn main() -> nest_rs_core::anyhow::Result<()> {
//! # let client = WsClient::for_test();
//!
//! let echoed = ChatGateway.dispatch(&client, "message", serde_json::json!({ "text": "hi" })).await;
//! assert!(matches!(echoed, WsReply::Reply(reply) if reply["text"] == "hi"));
//! # Ok(())
//! # }
//! ```
//!
//! # Every message declares its access posture
//!
//! `#[authorize(Action, Entity)]` or `#[public]`, and **neither is optional** —
//! a message with no posture does not compile. `#[authorize]` emits the class
//! gate (`nest_rs_authz::ws::authorize`) before the payload is deserialized and
//! the reply mask (`nest_rs_authz::ws::masked_reply_for`) around the returned
//! value. `#[public]` declares the message deliberately ungated: the guards bound
//! on the gateway and beside the message still run.
//!
//! The mask acts on the **serialized** reply, so a withheld column is absent from
//! the frame rather than an error. It fails closed on a missing ambient ability
//! and a body that cannot be reconciled with the entity. `unmasked` keeps the
//! gate and hands masking to the body. A masked handler returns a **literal**
//! `Result<T, E>`.
//!
//! # Return-type contract
//!
//! - `()` — send nothing.
//! - `T` — serialize as the reply on the request's event name.
//! - `Result<(), E>` / `Result<T, E>` — `Err(e)` becomes an error frame
//!   `{ "event": "<event>", "data": { "error": "<Display of e>" } }` and a
//!   `warn!(target: "nest_rs::ws", ...)` log.
//!
//! **`Display` for the error must be wire-safe**: [`Opaque`]'s `.opaque()?` logs
//! the real error at `error` on `nest_rs::ws` and hands the client a constant.
//! Reach for it on any failure a client is not owed an explanation for — a
//! `DbErr` above all, whose `Display` carries SQL.
//!
//! # Server→client push
//!
//! [`WsServer`] is the connection registry provided by [`WsModule`]. A handler
//! reaches it by declaring a `&`[`WsClient`] parameter (a reference,
//! distinguished from the owned payload).
//!
//! # Guards and lifecycle hooks
//!
//! - **Connection-level**: `#[use_guards]` on the gateway struct reuses the
//!   HTTP `Guard` trait and runs on the upgrade request.
//! - **Per-message**: `#[use_guards]` beside a `#[subscribe_message]` runs
//!   `Guard::check_ws_message` (global + per-message, deduped by `TypeId`)
//!   each time the event fires.
//!
//! `#[on_connect]` / `#[on_disconnect]` on the `#[messages]` impl block are the
//! lifecycle hooks; `on_disconnect` runs while the connection is still registered.
//!
//! # Versioning the mount
//!
//! `#[gateway(path = "/ws", version = "1")]` serves at `/v1/ws`, through
//! [`nest_rs_http::version_path`] as `#[controller]` does. Two gateways may share
//! a path under two versions; sharing a path *and* a version fails boot.
//!
//! **A gateway is selected by URI only**: under `<PREFIX>_HTTP__VERSIONING`'s
//! `header` or `media_type` a versioned gateway is still served at `/v1/ws`, and
//! `/ws` plus a version header reaches nothing.
//!
//! # Per-gateway namespacing
//!
//! [`WsServer`] is generic over a zero-sized namespace marker (default
//! [`Global`]). `#[gateway(namespace = MyNs)]` mounts against its own
//! `WsServer<MyNs>`, a separate registry also owned by [`WsModule`].
//!
//! # Ambient request data context
//!
//! The connection loop runs after the upgrade completes, so the task-locals an
//! HTTP request installs have unwound by the time a message handler runs. The
//! [`SocketContext`] seam captures per-connection state from the post-guard
//! upgrade request and re-installs it around each dispatch.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — the WebSocket edge: upgrades, connections, message dispatch.
pub const TARGET: &str = "nest_rs::ws";

/// The two targets this crate **emits on but does not own**, re-exported so
/// `nest-rs-ws-macros` reaches them through its own surface crate.
pub mod target {
    /// Layer composition and scope resolution, owned by `nest-rs-core`.
    pub use nest_rs_core::target::LAYERS;
    /// Mounted addresses, owned by `nest-rs-http`, which serves the upgrade.
    pub use nest_rs_http::target::ROUTES;
}

mod config;
mod context;
mod envelope;
mod error;
mod gateway;
mod guard;
mod module;
mod namespace;
mod opaque;
mod scope;
mod server;
pub mod unit;

pub use config::WsConfig;
pub use context::{BoxFuture, Captured, SocketContext};
pub use envelope::{
    ErrorReport, ErrorReportChain, ErrorReportFallback, ReplyOutcome, ReplyValue,
    ReplyValueFallback, WsEnvelope, WsError, WsReply,
};
pub use error::WsScopeError;
pub use gateway::{
    Gateway, GatewayEndpoint, WsDataFold, WsDataPipe, gateway_endpoint, resolve_ws_data_pipe,
};
pub use guard::{EventLayerTable, WsMessageCheck};
pub use module::{WsModule, WsSetup};
pub use namespace::{WsNamespaceEntry, WsNamespaces};
pub use opaque::Opaque;
pub use scope::Scoped;
pub use server::{ConnId, Global, Registry, WsClient, WsServer};

// Re-exported so macro-generated code resolves these through the framework.
pub use async_trait::async_trait;
#[doc(hidden)]
pub use serde_json;
#[doc(hidden)]
pub use tracing;

/// The RFC 6455 §7.4.1 status codes the gateway closes with.
pub use poem::web::websocket::CloseCode;

pub use poem;

// Re-exported so a WS-only crate needs no direct `nest-rs-http` dependency.
pub use nest_rs_http;

/// The wire-DTO shorthand, so a payload crossing this transport needs no
/// `serde` of its own.
pub use nest_rs_core::input;

/// Bind a `#[gateway]` impl block's message handlers.
///
/// ```
/// use nest_rs_core::Discoverable;
/// use nest_rs_ws::{Gateway, WsClient, WsReply, gateway, messages};
///
/// #[gateway(path = "/chat")]
/// #[derive(Default)]
/// struct ChatGateway;
///
/// #[messages]
/// impl ChatGateway {
///     #[subscribe_message("shout")]
///     #[public]
///     async fn shout(&self, text: String) -> String {
///         text.to_uppercase()
///     }
/// }
///
/// fn implements<T: Gateway + Discoverable>() {}
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// implements::<ChatGateway>();
///
/// let reply = ChatGateway
///     .dispatch(&WsClient::for_test(), "shout", serde_json::json!("hi"))
///     .await;
/// assert!(matches!(reply, WsReply::Reply(text) if text == "HI"));
/// # Ok(())
/// # }
/// ```
pub use nest_rs_ws_macros::messages;

/// Mark a struct as a WebSocket gateway.
///
/// ```
/// use nest_rs_core::module;
/// use nest_rs_ws::{WsModule, gateway, messages};
///
/// #[gateway(path = "/ws", version = "1")]
/// #[derive(Default)]
/// struct ChatGateway;
///
/// #[messages]
/// impl ChatGateway {
///     #[subscribe_message("ping")]
///     #[public]
///     async fn ping(&self) -> String {
///         "pong".into()
///     }
/// }
///
/// #[module(imports = [WsModule], providers = [ChatGateway])]
/// struct ChatModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
///
/// assert_eq!(ChatGateway::PATH, "/ws");
/// assert_eq!(ChatGateway::VERSION, Some("1"));
/// # let app = nest_rs_testing::TestApp::builder().module::<ChatModule>().build_ws().await?;
///
/// let mut socket = app.socket("/v1/ws").connect().await;
/// socket.send("ping", serde_json::Value::Null).await;
/// assert_eq!(socket.next_envelope().await["data"], "pong");
/// # app.shutdown().await?;
/// # Ok(())
/// # }
/// ```
///
/// `#[use_interceptors(...)]` / `#[use_filters(...)]` are **HTTP-only** and
/// rejected on a gateway at compile time:
///
/// ```compile_fail
/// use nest_rs_ws::gateway;
///
/// #[gateway(path = "/ws")]
/// #[use_filters(SomeFilter)]
/// struct BadGateway;
/// ```
pub use nest_rs_ws_macros::gateway;

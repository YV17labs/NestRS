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
//! # use nest_rs_guards::{Guard, GuardFor};
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
//! # impl GuardFor<nest_rs_http::HttpView> for AuthnGuard {}
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
//! gate (`nest_rs_authz::class_gate`) before the payload is deserialized and
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
//!   `{ "event": "<event>", "data": { "error": "…", "errors": … } }`, said by
//!   `e`'s **type**: a [`WsError`] as built; a
//!   [`ToProblem`](nest_rs_core::ToProblem) error as the problem it says
//!   ([`WsError::from_problem`]); a `Problem`, a pipe rejection or a decode
//!   failure its chain carries, in its own form, without a decode failure's
//!   value; anything else as
//!   [`OPAQUE_CLIENT_MESSAGE`](nest_rs_core::OPAQUE_CLIENT_MESSAGE).
//!
//! An answered error is filed at `warn` on `nest_rs::ws`; an opaque one, and the
//! chain behind a `5xx` problem, at `error`. An error's `Display` never reaches
//! the frame. [`Opaque`]'s `.opaque()?` says at the call site that a failure is
//! none of the client's business, whatever its type.
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
//!   `Guard::check` on the message's [`WsView`] (global + per-message, deduped
//!   by `TypeId`) each time the event fires.
//!
//! `#[use_interceptors]`, `#[use_exception_filters]` and `#[use_pipes]` bind on
//! the gateway, for each of its messages, and beside a `#[subscribe_message]`,
//! for that one.
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
//! # The pipeline, and units of work
//!
//! The upgrade runs as the HTTP transport's self-mount: its app-wide unit
//! stage, its guard stage (the app's pool and the gateway's `#[use_guards]`)
//! and the app's exception filters. Each message then runs through the same
//! pipeline as a route, as a `ws.message` unit: the unit stage, its guard stage
//! (the app's pool, the message's own guards, then the class gate its posture
//! declares), its exception filters (the message's, the gateway's, the app's),
//! its handler stage (the app's interceptors, the modules', the gateway's
//! `#[use_interceptors]`, the message's) and its tail — the data pipes, the
//! handler's own pipes, the call and the mask. A layer reads the message
//! through [`WsView`] and may answer it through [`WsView::respond`].
//!
//! The connection loop runs after the upgrade completes, under what the
//! upgrade carried into a [`Connection`](nest_rs_core::Continuation::Connection):
//! its trace, its actor, the ability its guards attached and every value that
//! crosses reach each hook and message, never its scope or its transaction.
//! Each message and each connection hook is a unit of work of its own, run
//! through the bound `nest_rs_database::UnitOfWork` and cut at its deadline: a
//! message commits unless it fails — an error frame, however a layer answered
//! it — a hook unless it unwound or was cut.
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
mod envelope;
mod gateway;
mod guard;
mod message;
mod module;
mod namespace;
mod opaque;
mod scope;
mod server;
pub mod unit;
mod view;

pub use config::WsConfig;
pub use envelope::{
    ErrorReport, ErrorReportChain, ErrorReportFallback, ErrorReportLocal, HandlerFailed,
    ReplyOutcome, ReplyValue, ReplyValueFallback, WsEnvelope, WsError, WsReply,
};
pub use gateway::Gateway;
pub use guard::denial_to_ws_error;
pub use module::{WsModule, WsSetup};
pub use namespace::WsNamespaces;
pub use opaque::Opaque;
pub use scope::Scoped;
pub use server::{ConnId, Global, Registry, WsClient, WsServer};
pub use view::WsView;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::gateway::{gateway_endpoint, wire_gateway};
    pub use crate::guard::{MessageGuards, message_guard_stage, refused_origin};
    pub use crate::message::{DeclaredMessage, MessageTable};
    pub use crate::namespace::WsNamespaceEntry;
    pub use crate::scope::scoped_parameter;
    pub use nest_rs_pipes::{GlobalPipe, PipeSpec};
}

// Re-exported so macro-generated code resolves these through the framework.
pub use async_trait::async_trait;
/// The JSON a message's `data` is, re-exported so a gateway, a guard or a
/// broadcast names `serde_json::Value` without its own dependency.
pub use serde_json;
/// The gateway's logger, re-exported so a gateway logs without its own
/// `tracing` dependency.
pub use tracing;

/// The status a socket closes with (RFC 6455 §7.4), which a test reads off
/// the socket it opened.
///
/// ```
/// use nest_rs_ws::CloseCode;
///
/// assert_eq!(u16::from(CloseCode::Away), 1001);
/// assert_eq!(CloseCode::from(1009), CloseCode::Size);
/// assert_eq!(u16::from(CloseCode::from(4409)), 4409);
/// ```
pub use nest_rs_http::__private::websocket::CloseCode;

// Re-exported so a WS-only crate needs no direct `nest-rs-http` dependency.
pub use nest_rs_http;

/// The wire-DTO shorthand, so a payload crossing this transport needs no
/// `serde` of its own.
pub use nest_rs_core::input;

/// Bind a `#[gateway]` impl block's message handlers.
///
/// A handler's parameters are read by their shape: a reference is the
/// `&`[`WsClient`], a [`Scoped<T>`] a request-scoped provider the boot checks,
/// and the one other parameter the payload `data` is decoded into.
/// [`Gateway::dispatch`] calls a handler on its own, behind its posture's
/// class gate — no guard, no layer — as a test does; on a socket each message
/// runs through its site.
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
/// `#[use_interceptors(...)]`, `#[use_exception_filters(...)]` and
/// `#[use_pipes(...)]` on the struct run around each of its messages;
/// `#[use_filters(...)]`, an untyped error mapper an exception filter replaced,
/// is refused at compile time:
///
/// ```compile_fail
/// use nest_rs_ws::gateway;
///
/// #[gateway(path = "/ws")]
/// #[use_filters(SomeFilter)]
/// struct BadGateway;
/// ```
pub use nest_rs_ws_macros::gateway;

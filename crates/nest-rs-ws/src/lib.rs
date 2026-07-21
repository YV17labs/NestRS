//! WebSocket gateways for nestrs.
//!
//! A `#[gateway]` struct with a `#[messages]` impl holds
//! `#[subscribe_message("event")]` handlers. Messages ride a JSON envelope
//! `{ "event": "...", "data": ... }`. Because a WS upgrade is an HTTP `GET`,
//! a gateway self-mounts on the existing HTTP transport — listing it in
//! `#[module(providers = [...])]` is the entire wiring; it inherits port,
//! CORS, TLS, and is governed by the boot-time access graph.
//!
//! ```ignore
//! #[gateway(path = "/ws")]
//! #[use_guards(AuthnGuard)]
//! struct ChatGateway {
//!     #[inject] svc: Arc<RoomService>,
//! }
//!
//! #[messages]
//! impl ChatGateway {
//!     #[subscribe_message("message")]
//!     async fn on_message(&self, msg: SendMessage) -> ChatMessage { /* ... */ }
//! }
//! ```
//!
//! # Return-type contract
//!
//! - `()` — send nothing.
//! - `T` — serialize as the reply on the request's event name.
//! - `Result<(), E>` / `Result<T, E>` — `Err(e)` becomes an error frame
//!   `{ "event": "<event>", "data": { "error": "<Display of e>" } }` and a
//!   `warn!(target: "nest_rs::ws", ...)` log.
//!
//! Detection is syntactic on the type's last path segment being `Result`: a
//! type alias over `Result` is **not** detected and would leak the error
//! variant on the wire. Always return `Result` (or `std::result::Result`)
//! directly. `Display` for the error must be wire-safe — avoid
//! `#[error(transparent)]` over an ORM/sqlx error.
//!
//! # Server→client push
//!
//! [`WsServer`] is the `@WebSocketServer` analog — a connection registry
//! provided by [`WsModule`]. A handler reaches it by declaring a
//! `&`[`WsClient`] parameter (a reference, distinguished from the owned
//! payload). Pushes funnel through a per-connection outbox drained by a
//! writer task, so the read loop never blocks on a slow `Sink`.
//!
//! # Guards and lifecycle hooks
//!
//! - **Connection-level**: `#[use_guards]` on the gateway struct reuses the
//!   HTTP `Guard` trait and runs on the upgrade request.
//! - **Per-message**: `#[use_guards]` beside a `#[subscribe_message]` runs
//!   the Layer System chain (global + per-message, deduped by `TypeId`)
//!   each time the event fires — same `Guard::check_ws_message` interface.
//!
//! `#[on_connect]` / `#[on_disconnect]` on the `#[messages]` impl block are
//! the `OnGatewayConnection` / `OnGatewayDisconnect` analogs; `on_disconnect`
//! runs while the connection is still registered.
//!
//! # Per-gateway namespacing
//!
//! [`WsServer`] is generic over a zero-sized namespace marker (default
//! [`Global`]). `#[gateway(namespace = MyNs)]` mounts against its own
//! `WsServer<MyNs>` — a separate registry the macro self-provides — so two
//! gateways isolate without sharing a registry.
//!
//! # Ambient request data context
//!
//! The connection loop runs in a task *after* the upgrade completes, so the
//! task-locals an HTTP request installs have unwound by the time a message
//! handler runs. The [`SocketContext`] seam captures opaque per-connection
//! state from the post-guard upgrade request and re-installs it around each
//! dispatch — this is how `nest_rs_seaorm::ws` re-binds executor + ability
//! per message without `nest-rs-ws` depending on the ORM or authz.
#![warn(missing_docs)]

mod config;
mod context;
mod envelope;
mod gateway;
mod guard;
mod module;
mod scope;
mod server;

pub use config::WsConfig;
pub use context::{BoxFuture, Captured, SocketContext};
pub use envelope::{WsEnvelope, WsReply};
pub use gateway::{
    Gateway, GatewayEndpoint, WsDataFold, WsDataPipe, gateway_endpoint, resolve_ws_data_pipe,
};
pub use guard::{EventLayerTable, WsMessageCheck};
pub use module::{WsModule, WsSetup};
/// Per-message accessor for `#[injectable(scope = request)]` providers inside a
/// WS message handler — the WS mirror of `nest_rs_http::Scoped<T>`.
pub use scope::{Scoped, WsScopeError};
pub use server::{ConnId, Global, Registry, WsClient, WsServer};

// Re-exported so macro-generated code resolves these through the framework.
pub use async_trait::async_trait;
// Hidden: macro plumbing, not public API — like `nest-rs-queue`'s treatment of
// the same two re-exports.
#[doc(hidden)]
pub use serde_json;
#[doc(hidden)]
pub use tracing;

pub use poem;

pub use nest_rs_ws_macros::messages;

/// The gateway decorator. `#[use_interceptors(...)]` / `#[use_filters(...)]`
/// are **HTTP-only** — the per-message WS seam is reserved but not invoked, so
/// binding one on a gateway is rejected at compile time instead of silently
/// doing nothing:
///
/// ```compile_fail
/// use nest_rs_ws::gateway;
///
/// #[gateway(path = "/ws")]
/// #[use_filters(SomeFilter)]
/// struct BadGateway;
/// ```
pub use nest_rs_ws_macros::gateway;

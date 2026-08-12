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
//!     #[public]
//!     async fn on_message(&self, msg: SendMessage) -> ChatMessage { /* ... */ }
//!
//!     #[subscribe_message("rooms.list")]
//!     #[authorize(Read, rooms::Entity)]  // class gate + reply mask, both emitted
//!     async fn rooms(&self) -> Result<Vec<Room>, ServiceError> { /* ... */ }
//! }
//! ```
//!
//! # Every message declares its access posture
//!
//! `#[authorize(Action, Entity)]` or `#[public]`, and **neither is optional** —
//! a message with no posture does not compile, the same rule a `#[query]` and a
//! `#[tool]` carry. `#[authorize]` emits the class gate
//! (`nest_rs_authz::ws::authorize`) before the payload is deserialized and the
//! reply mask (`nest_rs_authz::ws::masked_reply_for`) around the returned value,
//! so a handler answering with entity rows writes no masking call. `#[public]`
//! declares the message deliberately ungated: the guards bound on the gateway and
//! beside the message still run.
//!
//! The mask acts on the **serialized** reply, so a withheld column is absent from
//! the frame rather than an error — HTTP's behaviour, since an envelope promises no
//! schema. Fail-closed is reserved for a missing ambient ability and a body that
//! cannot be reconciled with the entity. `unmasked` keeps the gate and hands
//! masking to the body, for a shape the round-trip cannot see through.
//!
//! One compile rule, and its reason is the alias hazard below rather than masking:
//! a masked handler returns a **literal** `Result<T, E>`.
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
//! directly.
//!
//! **`Display` for the error must be wire-safe**, and [`Opaque`] is the seam that
//! makes it so without the handler having to think about it: `.opaque()?` logs the
//! real error at `error` on `nest_rs::ws` and hands the client a constant. Reach
//! for it on any failure a client is not owed an explanation for — a `DbErr` above
//! all, whose `Display` carries SQL. A rejection the client *can* act on is the
//! opposite case and travels as itself.
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
//! # Versioning the mount
//!
//! `#[gateway(path = "/ws", version = "1")]` serves at `/v1/ws`. It is the same
//! declaration `#[controller]` takes and it resolves through the same
//! [`nest_rs_http::version_path`], because a gateway's mount *is* an address the
//! client selects — the edges where it is not (GraphQL, MCP, queues) refuse the
//! argument at compile time rather than accept a version nothing could apply.
//!
//! A gateway owns its mount, so the version is part of the identity the boot
//! checks: `#[gateway(path = "/ws", version = "1")]` and `version = "2"` are two
//! mounts that both boot and serve their own message tables, while two gateways
//! sharing a path *and* a version still fail boot naming both.
//!
//! **A gateway is selected by URI only.** `NESTRS_HTTP__VERSIONING` rewrites
//! *controller* paths in front of routing, and the selector learns its prefixes
//! from controllers alone — a self-mount is version-neutral to it. So under
//! `header` or `media_type` a versioned gateway is still served at `/v1/ws`
//! (rather than refused as a URI form), and `/ws` + a version header reaches
//! nothing. That is the honest shape for this edge: a browser cannot set headers
//! on a `WebSocket` handshake anyway.
//!
//! # Per-gateway namespacing
//!
//! [`WsServer`] is generic over a zero-sized namespace marker (default
//! [`Global`]). `#[gateway(namespace = MyNs)]` mounts against its own
//! `WsServer<MyNs>` — a separate registry, also owned by [`WsModule`] (see
//! `crate::namespace`) — so two gateways isolate without sharing a registry.
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
mod namespace;
mod opaque;
mod scope;
mod server;

pub use config::WsConfig;
pub use context::{BoxFuture, Captured, SocketContext};
pub use envelope::{ReplyValue, ReplyValueFallback, WsEnvelope, WsError, WsReply};
pub use gateway::{
    Gateway, GatewayEndpoint, WsDataFold, WsDataPipe, gateway_endpoint, resolve_ws_data_pipe,
};
pub use guard::{EventLayerTable, WsMessageCheck};
pub use module::{WsModule, WsSetup};
pub use namespace::{WsNamespaceEntry, WsNamespaces};
pub use opaque::Opaque;
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

// Re-exported so `#[messages]`-generated discovery metadata
// (`HttpEndpointMeta` — the WS upgrade is an HTTP GET) resolves through this
// crate: a WS-only gateway crate needs no direct `nest-rs-http` dependency.
pub use nest_rs_http;

/// The wire-DTO shorthand — same decorator the HTTP layer uses, re-exported
/// here so a payload crossing this transport needs no `serde` of its own.
pub use nest_rs_core::input;

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

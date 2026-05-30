//! WebSocket gateways for nestrs.
//!
//! A **gateway** is the WebSocket counterpart of an HTTP controller: a
//! `#[gateway]`-decorated struct whose `#[messages]` impl block holds
//! `#[subscribe_message("event")]` handlers (the `@WebSocketGateway` /
//! `@SubscribeMessage` analogs). Messages ride a JSON envelope
//! `{ "event": "...", "data": ... }`: the handler's owned parameter is
//! deserialized from `data`, and its return value is serialized back under the
//! same event name (a `()` return sends nothing).
//!
//! Because a WebSocket upgrade *is* an HTTP `GET`, a gateway does not open a
//! second server: `#[messages]` attaches an [`nestrs_http::HttpEndpointMeta`] so
//! the gateway **self-mounts on the existing HTTP transport** at its `path`,
//! exactly as a GraphQL or OpenAPI endpoint does. Listing a gateway in a
//! `#[module(providers = [...])]` is all the wiring there is — it inherits the
//! transport's port, CORS and TLS, and is governed by the boot-time access
//! graph like any other provider.
//!
//! ```ignore
//! #[gateway(path = "/ws")]
//! #[use_guards(AuthGuard)]            // connection-level, run on the upgrade
//! struct ChatGateway {
//!     #[inject] room: Arc<RoomService>,
//! }
//!
//! #[messages]
//! impl ChatGateway {
//!     #[subscribe_message("message")]
//!     async fn on_message(&self, msg: SendMessage) -> ChatMessage { /* ... */ }
//! }
//! ```
//!
//! # Server→client push
//!
//! Beyond replying on its own socket, a gateway pushes to *other* clients
//! through [`WsServer`] — the `@WebSocketServer` analog, a connection registry
//! provided as a singleton by [`WsModule`]. Import `WsModule` and a service can
//! `#[inject] Arc<WsServer>` to broadcast in reaction to a domain event; a
//! handler reaches the same registry by declaring a `&`[`WsClient`] parameter
//! (the `@ConnectedSocket` analog, distinguished from the owned payload by being
//! a reference, exactly as a `#[field]` resolver tells a `&DataLoader` from its
//! arguments):
//!
//! ```ignore
//! #[subscribe_message("join")]
//! async fn join(&self, room: JoinRoom, client: &nestrs_ws::WsClient) {
//!     client.join(room.name);                 // address a room later
//! }
//!
//! #[subscribe_message("say")]
//! async fn say(&self, msg: Say, client: &nestrs_ws::WsClient) {
//!     let _ = client.to(&msg.room, "said", &msg);   // push to the room
//! }
//! ```
//!
//! Pushes (a handler's reply, a broadcast, a room emit) all funnel through one
//! per-connection outbox drained by a writer task, so the read loop never blocks
//! on a slow `Sink` and a service can reach a client mid-handler.
//!
//! # Guards and lifecycle hooks
//!
//! Guards bind at two scopes. A **connection-level** guard — `#[use_guards]` on
//! the gateway struct — reuses the HTTP [`Guard`] trait and runs on the upgrade
//! request, so a rejected handshake never opens the socket. A **per-message**
//! guard — `#[use_guards]` beside a `#[subscribe_message]` — implements
//! [`MessageGuard`] (its context is the message, not a `poem::Request`) and gates
//! that one event; an `Err` reply short-circuits to an error frame and the
//! handler never runs.
//!
//! A gateway may also implement the connection **lifecycle hooks** — an
//! `#[on_connect]` / `#[on_disconnect]` method on the `#[messages]` impl block
//! (the `OnGatewayConnection` / `OnGatewayDisconnect` analogs). Each takes
//! `&self` (the gateway is a singleton) and optionally the connecting
//! `&`[`WsClient`]; `on_connect` runs before the first message, `on_disconnect`
//! while the connection is still registered.
//!
//! # Deliberate limits of this cut
//!
//! - **No ambient request data context.** The connection loop runs in a task
//!   *after* the upgrade request completes, so the HTTP request scope, the ORM
//!   executor and the authz ability task-locals do **not** reach a handler — the
//!   same constraint a `#[dataloader]` batch has. A gateway handler injects an
//!   `Arc<DatabaseConnection>` and queries it directly.
//! - **One registry per app, not per gateway.** The flat container keys
//!   [`WsServer`] by type, so every gateway shares one registry — a `broadcast`
//!   reaches all of them. Scope with rooms; per-gateway namespacing (the way a
//!   second `OAuth2Client` needs a newtype) is not built.

mod envelope;
mod gateway;
mod guard;
mod module;
mod server;

pub use envelope::{WsEnvelope, WsReply};
pub use gateway::{gateway_endpoint, Gateway, GatewayEndpoint};
pub use guard::{MessageGuard, MessageGuardTable};
pub use module::WsModule;
pub use server::{ConnId, WsClient, WsServer};

// Re-exported so `#[messages]`-generated code resolves these through the
// framework: the dispatcher is `#[nestrs_ws::async_trait]`, payloads go through
// `nestrs_ws::serde_json`, and `#[gateway]`'s guard wrapping names
// `nestrs_ws::EndpointExt`.
pub use async_trait::async_trait;
pub use nestrs_middleware::{EndpointExt, Guard};
pub use serde_json;

// `#[gateway]`-generated guard wrapping names poem types through the framework
// (`::nestrs_ws::poem::*`), so a WebSocket-only app needs no direct poem dep.
pub use poem;

/// WebSocket decorators (`#[gateway]`, `#[messages]`, and the inert
/// `#[subscribe_message]` consumed by `#[messages]`), defined in
/// `nestrs-ws-macros` and surfaced here so apps write `nestrs_ws::gateway` etc.
pub use nestrs_ws_macros::{gateway, messages};

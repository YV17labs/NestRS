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
//! # Return-type contract
//!
//! A `#[subscribe_message]` handler may return one of four shapes; the macro
//! picks the dispatch shape syntactically:
//!
//! - `()` (or no return) — send nothing.
//! - `T` — serialize as the reply on the request's event name.
//! - `Result<(), E>` — `Ok(())` sends nothing; `Err(e)` becomes an error frame
//!   `{ "event": "<event>", "data": { "error": "<Display of e>" } }`, and a
//!   `warn!(target: "nestrs::ws", error = ?e, ...)` is emitted server-side.
//! - `Result<T, E>` — `Ok(t)` is serialized as the reply; `Err(e)` produces
//!   the same error frame + log as above.
//!
//! **`E: Display` is required** for the `Result` shapes (the wire frame uses
//! `format!("{}", e)`). The detection is purely syntactic on the type's last
//! path segment being `Result`: a type alias such as
//! `pub type Outcome<T> = Result<T, MyError>;` does **not** classify as
//! `Result` — the macro treats it as a plain `T` and serde-serializes the
//! enum, leaking the error variant on the wire. Always return `Result` (or
//! `std::result::Result`) directly.
//!
//! **Display discipline.** The `Err(e)` ships `format!("{}", e)` to the wire;
//! `Display` for that error must be wire-safe. Avoid `#[error(transparent)]`
//! over an ORM/sqlx error (it would forward SQL fragments to the client); use
//! a fixed `#[error("...")]` per variant, and rely on the structured `?e`
//! Debug log for server-side observability — `domain::users::UserError` and
//! `domain::orgs::OrgError` are the reference shapes.
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
//! # Per-gateway namespacing
//!
//! [`WsServer`] is generic over a zero-sized namespace marker (default
//! [`Global`], the registry [`WsModule`] provides). `#[gateway(namespace = MyNs)]`
//! mounts the gateway against its own `WsServer<MyNs>` — a separate registry the
//! macro self-provides — so two gateways isolate without sharing a registry (a
//! `broadcast` on one never reaches the other's clients). The handler surface is
//! untouched: a handler still takes `&`[`WsClient`], which carries the registry
//! type-erased as [`Registry`]. A service that pushes to a namespaced registry
//! injects `Arc<WsServer<MyNs>>` and must list it in a module's `providers`.
//!
//! # Ambient request data context
//!
//! The connection loop runs in a task *after* the upgrade request completes, so
//! the task-locals an HTTP request installs (the ORM executor, the authz
//! ability) have already unwound by the time a message handler runs — the same
//! constraint a `#[dataloader]` batch has. The [`SocketContext`] seam closes
//! that without `nestrs-ws` depending on the ORM or authz: it captures opaque
//! per-connection state from the post-guard upgrade request and re-installs it
//! around each dispatch. The `nestrs_database::ws` bridge implements it to
//! bind the request executor and the caller's ability, so a gateway handler
//! uses `Repo` with row-level filtering exactly like a controller. With no
//! bridge bound a handler still injects an `Arc<DatabaseConnection>` and
//! queries it directly.

mod context;
mod envelope;
mod gateway;
mod guard;
mod module;
mod server;

pub use context::{BoxFuture, Captured, SocketContext};
pub use envelope::{WsEnvelope, WsReply};
pub use gateway::{gateway_endpoint, Gateway, GatewayEndpoint};
pub use guard::{MessageGuard, MessageGuardTable};
pub use module::WsModule;
pub use server::{ConnId, Global, Registry, WsClient, WsServer};

// Re-exported so `#[messages]`-generated code resolves these through the
// framework: the dispatcher is `#[nestrs_ws::async_trait]`, payloads go through
// `nestrs_ws::serde_json`, `#[gateway]`'s guard wrapping names
// `nestrs_ws::EndpointExt`, and a Result-returning `#[subscribe_message]`'s Err
// branch logs through `nestrs_ws::tracing::warn!` so users without a direct
// `tracing` dep still get the warn event.
pub use async_trait::async_trait;
pub use nestrs_middleware::{EndpointExt, Guard};
pub use serde_json;
pub use tracing;

// `#[gateway]`-generated guard wrapping names poem types through the framework
// (`::nestrs_ws::poem::*`), so a WebSocket-only app needs no direct poem dep.
pub use poem;

/// WebSocket decorators (`#[gateway]`, `#[messages]`, and the inert
/// `#[subscribe_message]` consumed by `#[messages]`), defined in
/// `nestrs-ws-macros` and surfaced here so apps write `nestrs_ws::gateway` etc.
pub use nestrs_ws_macros::{gateway, messages};

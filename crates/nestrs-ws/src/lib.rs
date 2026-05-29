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
//! # Deliberate limits of this first cut
//!
//! - **Guards bind at the connection level** (on the upgrade request), not
//!   per message. A rejected handshake never opens the socket.
//! - **No ambient request data context.** The connection loop runs in a task
//!   *after* the upgrade request completes, so the HTTP request scope, the ORM
//!   executor and the authz ability task-locals do **not** reach a handler — the
//!   same constraint a `#[dataloader]` batch has. A gateway handler injects an
//!   `Arc<DatabaseConnection>` and queries it directly.
//! - **Request/response only.** A handler replies on its own socket; server→
//!   client broadcast and a connection registry (the `@WebSocketServer` analog)
//!   are the next brick — the one SSE and GraphQL subscriptions will reuse.
//! - **No lifecycle hooks** (`OnGatewayConnection`/`Disconnect`) yet.

mod envelope;
mod gateway;

pub use envelope::{WsEnvelope, WsReply};
pub use gateway::{gateway_endpoint, Gateway, GatewayEndpoint};

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

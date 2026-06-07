//! WebSocket gateway decorator macros. Generated code uses absolute paths so
//! this crate does not depend on the surface crates.
//!
//! `#[subscribe_message("event")]`, `#[on_connect]`, `#[on_disconnect]` are
//! inert attributes consumed by `#[messages]`, same shape as the HTTP verb
//! attributes consumed by `#[routes]`.

use proc_macro::TokenStream;

mod attr;
mod gateway;
mod messages;

/// `#[gateway(path = "/ws")]` — the `@WebSocketGateway` analog. Generates
/// `from_container`, `pub const PATH`, and the inherent helpers `#[messages]`
/// reads back.
///
/// `namespace = MarkerType` mounts against `WsServer<MarkerType>` — a
/// self-provided isolated registry. Omitted, uses `Global` from `WsModule`.
///
/// `#[use_guards(...)]` on the struct = connection-level guards, run on the
/// HTTP upgrade request so a rejected handshake never opens the socket. The
/// `Discoverable` impl is emitted by `#[messages]` (it needs the message
/// table).
#[proc_macro_attribute]
pub fn gateway(args: TokenStream, input: TokenStream) -> TokenStream {
    gateway::gateway(args, input)
}

/// Bind a `#[gateway]` impl block's message handlers. Each
/// `#[subscribe_message("event")]` method handles `{ "event": "...", "data":
/// ... }`; the owned parameter is deserialized from `data`, the return value
/// serialized back under the same event (`()` => no reply).
///
/// `#[use_guards(...)]` beside a handler binds per-message guards that the
/// Layer System dedups against the global chain. `#[on_connect]` /
/// `#[on_disconnect]` are the lifecycle-hook analogs — `&self` with an
/// optional `&WsClient`.
///
/// Emits `Gateway` (dispatcher + hooks) and `Discoverable` — the latter
/// attaches an `HttpEndpointMeta` so the gateway self-mounts on the HTTP
/// transport at `PATH`.
#[proc_macro_attribute]
pub fn messages(args: TokenStream, input: TokenStream) -> TokenStream {
    messages::messages(args, input)
}

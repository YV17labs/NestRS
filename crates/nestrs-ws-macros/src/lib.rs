//! WebSocket gateway decorator macros, re-exported by `nestrs-ws` so apps write
//! `nestrs_ws::gateway` etc. The generated code uses absolute paths
//! (`::nestrs_ws::*`, `::nestrs_http::*`, `::poem::*`, `::nestrs_core::*`), so
//! this crate does not depend on those crates — they resolve at the call site.
//!
//! Each `#[proc_macro_attribute]` here is a thin entry the language requires at
//! the crate root; the implementation lives in the topical submodules
//! (`gateway`, `messages`, `attr`). `#[subscribe_message("event")]`,
//! `#[on_connect]`, and `#[on_disconnect]` are **not** macros of their own — they
//! are inert attributes that `#[messages]` consumes and strips off the impl
//! block, exactly as the HTTP verb attributes (`#[get]`, …) are consumed by
//! `#[routes]`.

use proc_macro::TokenStream;

mod attr;
mod gateway;
mod messages;

/// `#[gateway(path = "/ws")]` — the WebSocket gateway struct decorator, paired
/// with `#[messages]` on the impl block. The NestJS `@WebSocketGateway` analog.
///
/// Generates a `from_container(&Container) -> Self` constructor (its `#[inject]`
/// fields pulled from the container), a `pub const PATH: &'static str` used by
/// `#[messages]` as the mount path, and the inherent helpers `#[messages]` reads
/// back (`__nestrs_injected` for the access graph, `__nestrs_gateway_layers` for
/// the connection-level guards).
///
/// A `#[use_guards(GuardA, GuardB)]` attribute placed *on the gateway struct*
/// (just below `#[gateway]`) declares **connection-level guards** — the same
/// `Guard` providers HTTP controllers use, resolved from the container, run on
/// the WebSocket *upgrade* request (so a rejected handshake never opens the
/// socket). First listed runs outermost. (Beside a `#[subscribe_message]`,
/// `#[use_guards]` instead binds **per-message** `MessageGuard`s — see
/// `#[messages]`.) `#[use_guards]` must sit below `#[gateway]`: it is an inert
/// attribute that `#[gateway]` consumes and strips.
///
/// The `Discoverable` impl is emitted by `#[messages]` rather than here — it
/// needs the message table that `#[messages]` collects.
#[proc_macro_attribute]
pub fn gateway(args: TokenStream, input: TokenStream) -> TokenStream {
    gateway::gateway(args, input)
}

/// Bind a `#[gateway]` impl block's message handlers to incoming WebSocket
/// events.
///
/// Applied to an `impl` block belonging to a `#[gateway]`-marked struct. Each
/// method tagged `#[subscribe_message("event")]` (the `@SubscribeMessage`
/// analog) handles a frame whose JSON envelope `{ "event": "...", "data": ... }`
/// names that event: the handler's owned parameter is deserialized from `data`,
/// and its return value is serialized back to the client under the same event
/// name. A handler returning `()` sends no reply.
///
/// A handler may also carry `#[use_guards(GuardA, GuardB)]` (per-message
/// `MessageGuard`s, resolved from the container — an `Err` short-circuits to an
/// error frame before the handler runs), and the impl block may declare
/// `#[on_connect]` / `#[on_disconnect]` lifecycle hooks (the
/// `OnGatewayConnection` / `OnGatewayDisconnect` analogs) — each `&self`, with an
/// optional `&WsClient` parameter.
///
/// Emits two impls on the gateway:
/// - `nestrs_ws::Gateway` — the per-connection message dispatcher, plus any
///   declared lifecycle-hook overrides.
/// - `nestrs_core::Discoverable` — attaches an `nestrs_http::HttpEndpointMeta`
///   so the gateway self-mounts on the HTTP transport's route tree at `PATH`
///   (the upgrade is an HTTP `GET`), exactly as a GraphQL or OpenAPI endpoint
///   does. The transport iterates these metas at boot — no `main.rs` wiring.
#[proc_macro_attribute]
pub fn messages(args: TokenStream, input: TokenStream) -> TokenStream {
    messages::messages(args, input)
}

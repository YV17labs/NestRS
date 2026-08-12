//! WebSocket gateway decorator macros. Generated code uses absolute paths so
//! this crate does not depend on the surface crates.
//!
//! `#[subscribe_message("event")]`, `#[on_connect]`, `#[on_disconnect]` are
//! inert attributes consumed by `#[messages]`, same shape as the HTTP verb
//! attributes consumed by `#[routes]`.
#![warn(missing_docs)]

use proc_macro::TokenStream;

mod gateway;
mod messages;

/// `#[gateway(path = "/ws")]` — the `@WebSocketGateway` analog. Generates
/// `from_container`, `pub const PATH` / `pub const VERSION`, and the inherent
/// helpers `#[messages]` reads back.
///
/// `version = "1"` mounts the gateway at `/v1/ws` instead of `/ws` — the same
/// declaration `#[controller]` takes, resolved through the same
/// `nest_rs_http::version_path`, because a gateway's mount is an address a
/// client selects. Two gateways may share a `path` under different versions;
/// two on the same path *and* version fail boot naming both.
///
/// `namespace = MarkerType` mounts against `WsServer<MarkerType>` — a
/// self-provided isolated registry. Omitted, uses `Global` from `WsModule`.
///
/// `#[use_guards(...)]` on the struct = connection-level guards, run on the
/// HTTP upgrade request so a rejected handshake never opens the socket. The
/// `Discoverable` impl is emitted by `#[messages]` (it needs the message
/// table).
///
/// # Expands to
///
/// The struct unchanged, plus inherent items: `PATH`, `VERSION`,
/// `from_container`, `__nestrs_mount_path` (the two folded through
/// `version_path`), `__nestrs_injected` (inject keys + connection guards),
/// `__nestrs_registry` / `__nestrs_provide_registry` (resolve/provide the
/// `WsServer<Ns>`), and `__nestrs_gateway_layers` (wraps the endpoint with the
/// connection-level guard chain, deduped against the global chain). No
/// `Discoverable` here — `#[messages]` emits it.
///
/// ```ignore
/// pub struct ChatGateway { /* … */ }
/// impl ChatGateway {
///     pub const PATH: &'static str = "/ws";
///     pub const VERSION: Option<&'static str> = Some("1");
///     pub fn __nestrs_mount_path() -> String { /* "/v1/ws" */ }
///     fn from_container(c: &::nest_rs_core::Container) -> Self { /* … */ }
///     pub fn __nestrs_injected() -> Vec<TypeId> { /* … */ }
///     pub fn __nestrs_registry(c) -> Arc<::nest_rs_ws::WsServer<Ns>> { /* … */ }
///     pub fn __nestrs_provide_registry(b) -> ContainerBuilder { /* … */ }
///     pub fn __nestrs_gateway_layers<E>(c, ep) -> BoxEndpoint { /* guard layers */ }
/// }
/// ```
#[proc_macro_attribute]
pub fn gateway(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(gateway::gateway(args, input).into()).into()
}

/// Bind a `#[gateway]` impl block's message handlers. Each
/// `#[subscribe_message("event")]` method handles `{ "event": "...", "data":
/// ... }`; the owned parameter is deserialized from `data`, the return value
/// serialized back under the same event (`()` => no reply).
///
/// Each handler declares an access posture — `#[authorize(Action, Entity)]` or
/// `#[public]` — and the expansion runs, in order: the per-message guard chain,
/// the class gate the posture emits, the argument pipes, the call, and the reply
/// mask. Mandatory, so a message nobody decided a posture for does not compile.
///
/// `#[use_guards(...)]` beside a handler binds per-message guards that the
/// Layer System dedups against the global chain. `#[on_connect]` /
/// `#[on_disconnect]` are the lifecycle-hook analogs — `&self` with an
/// optional `&WsClient`.
///
/// Emits `Gateway` (dispatcher + hooks) and `Discoverable` — the latter
/// attaches an `HttpEndpointMeta` so the gateway self-mounts on the HTTP
/// transport at its effective path (`PATH`, with `VERSION` folded in).
///
/// # Expands to
///
/// The impl unchanged, an `impl Gateway` whose `dispatch` matches the event
/// name to the right handler (deserializing `data`, serializing the reply) and
/// carries the `on_connect`/`on_disconnect` overrides, and an `impl
/// Discoverable` whose `register` attaches an `HttpEndpointMeta` that
/// self-mounts at `__nestrs_mount_path()` and composes the per-event guard
/// chains (global + per-message, deduped) once at mount.
///
/// ```ignore
/// #[::nest_rs_ws::async_trait]
/// impl ::nest_rs_ws::Gateway for ChatGateway {
///     async fn dispatch(&self, client, event, data) -> ::nest_rs_ws::WsReply {
///         match event { "send" => { /* deser data → call handler → reply */ } _ => unknown }
///     }
///     async fn on_connect(&self, client) { /* … */ }    // if present
/// }
/// impl ::nest_rs_core::Discoverable for ChatGateway {
///     fn register(b) -> ContainerBuilder { /* attach_meta::<_, HttpEndpointMeta>(… self-mount at PATH …) */ }
/// }
/// ```
#[proc_macro_attribute]
pub fn messages(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(messages::messages(args, input).into()).into()
}

//! WebSocket gateway decorator macros, re-exported by `nest-rs-ws`.
//!
//! `#[subscribe_message("event")]`, `#[on_connect]`, `#[on_disconnect]` are
//! inert attributes consumed by `#[messages]`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod gateway;
mod messages;

/// `#[gateway(path = "/ws")]` generates `from_container`, `pub const PATH` /
/// `pub const VERSION`, and the inherent helpers `#[messages]` reads back.
///
/// `version = "1"` mounts the gateway at `/v1/ws` instead of `/ws`, as
/// `#[controller]` does. Two gateways may share a `path` under different
/// versions; two on the same path *and* version fail boot naming both.
///
/// `namespace = MarkerType` mounts against `WsServer<MarkerType>` — a
/// self-provided isolated registry. Omitted, uses `Global` from `WsModule`.
///
/// `#[use_guards(...)]` on the struct = connection-level guards, run on the
/// HTTP upgrade request so a rejected handshake never opens the socket.
///
/// # Expands to
///
/// The struct unchanged, plus `PATH`, `VERSION`, a private `from_container`,
/// and the hidden helpers `#[messages]` reads. `#[messages]` emits the
/// `Discoverable` impl.
#[proc_macro_attribute]
pub fn gateway(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(gateway::gateway(args, input).into()).into()
}

/// Each `#[subscribe_message("event")]` method handles `{ "event": "...", "data":
/// ... }`; the owned parameter is deserialized from `data`, the return value
/// serialized back under the same event (`()` => no reply).
///
/// Each handler must declare an access posture — `#[authorize(Action, Entity)]`
/// or `#[public]` — and the expansion runs, in order: the per-message guard
/// chain, the class gate the posture emits, the argument pipes, the call, and
/// the reply mask.
///
/// `#[use_guards(...)]` beside a handler binds per-message guards, deduped
/// against the global chain. `#[on_connect]` / `#[on_disconnect]` are the
/// lifecycle hooks — `&self` with an optional `&WsClient`.
///
/// # Expands to
///
/// The impl unchanged, an `impl Gateway` whose `dispatch` matches the event
/// name to its handler and carries the lifecycle hooks, and an `impl
/// Discoverable` attaching the `HttpEndpointMeta` that self-mounts the gateway
/// at `PATH` with `VERSION` folded in.
#[proc_macro_attribute]
pub fn messages(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(messages::messages(args, input).into()).into()
}

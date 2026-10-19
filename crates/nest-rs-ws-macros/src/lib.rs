//! WebSocket gateway decorator macros, re-exported by `nest-rs-ws`.
//!
//! `#[subscribe_message("event")]`, `#[on_connect]`, `#[on_disconnect]` are
//! inert attributes consumed by `#[messages]`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod gateway;
mod messages;

/// `#[gateway(path = "/ws")]` generates a hidden constructor, `pub const PATH`
/// / `pub const VERSION`, and the inherent helpers `#[messages]` reads back.
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
/// `#[use_interceptors(...)]`, `#[use_exception_filters(...)]` and
/// `#[use_pipes(...)]` on the struct run around, and over the `data` of, each
/// of its messages. A `#[meta(EXPR)]` on the struct is declared for every
/// message and hook.
///
/// # Expands to
///
/// The struct unchanged, plus `PATH`, `VERSION`, a hidden constructor,
/// and the hidden helpers `#[messages]` reads. `#[messages]` emits the
/// `Discoverable` impl.
#[proc_macro_attribute]
pub fn gateway(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(gateway::gateway(args, input).into()).into()
}

/// Each `#[subscribe_message("event")]` method handles `{ "event": "...", "data":
/// ... }`; the payload parameter is deserialized from `data`, the return value
/// serialized back under the same event (`()` => no reply).
///
/// A handler's parameters are read by their shape, since a macro reads tokens:
/// a reference is the `&WsClient`, a type named `Scoped` a request-scoped
/// provider — resolved by a function of `nest_rs_ws` that returns its own
/// `Scoped`, so a namesake does not compile — and the one other parameter the
/// payload.
///
/// Each handler must declare an access posture — `#[authorize(Action, Entity)]`
/// or `#[public]` — and each message runs as a site of the pipeline, in order:
/// the guard stage (the app's pool, the message's guards, the class gate the
/// posture emits), the exception filters and the interceptors (the message's
/// and the gateway's beside the app's), the data pipes, the argument pipes, the
/// call, and the reply mask.
///
/// `#[use_guards(...)]` beside a handler binds for that message, deduped
/// against the app's pool: the gateway's guards run at the upgrade.
/// `#[use_interceptors(...)]`, `#[use_exception_filters(...)]` and
/// `#[use_pipes(...)]` beside it bind for it, deduped against the gateway's and
/// the app's.
/// `#[on_connect]` / `#[on_disconnect]` are the lifecycle hooks — `&self` with
/// an optional `&WsClient`. `#[meta(EXPR)]` (repeatable) on a handler or a hook
/// declares a value for it, read over the gateway's of the same type.
///
/// # Expands to
///
/// The impl unchanged, a `static` site per message, an `impl Gateway` whose
/// handle matches the event name to its handler, whose gate lookup matches it
/// to its posture's class gate, and which carries the lifecycle hooks, and an
/// `impl Discoverable` attaching the `HttpEndpointMeta` that
/// self-mounts the gateway at `PATH` with `VERSION` folded in, and the wiring
/// that checks the gateway at boot.
#[proc_macro_attribute]
pub fn messages(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(messages::messages(args, input).into()).into()
}

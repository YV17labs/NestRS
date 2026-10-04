//! WebSocket gateway decorator macros. Generated code uses absolute paths so
//! this crate does not depend on the surface crates.
//!
//! `#[subscribe_message("event")]`, `#[on_connect]`, `#[on_disconnect]` are
//! inert attributes consumed by `#[messages]`, same shape as the HTTP verb
//! attributes consumed by `#[routes]`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod gateway;
mod messages;

/// `#[gateway(path = "/ws")]` generates `from_container`, `pub const PATH` /
/// `pub const VERSION`, and the inherent helpers `#[messages]` reads back.
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
/// The struct unchanged, plus inherent items: `PATH`, `VERSION`, a private
/// `from_container`, and hidden helpers that fold the two into the mount path,
/// list the injected keys and connection guards, resolve the namespace's
/// `WsServer<Ns>`, and wrap the endpoint in the connection-level guard chain. No
/// `Discoverable` here — `#[messages]` emits it.
#[proc_macro_attribute]
pub fn gateway(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(gateway::gateway(args, input).into()).into()
}

/// Each `#[subscribe_message("event")]` method handles `{ "event": "...", "data":
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
#[proc_macro_attribute]
pub fn messages(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(messages::messages(args, input).into()).into()
}

# nestrs-ws

WebSocket gateways for [nestrs](https://nestrs.dev), built on
[poem's `WebSocket`](https://docs.rs/poem/latest/poem/web/websocket/index.html)
upgrade. A `#[gateway] + #[messages]` impl decoratively maps
`#[subscribe_message("event")]` handlers over the JSON envelope
`{ event, data }`. Because a WS upgrade is an HTTP `GET`, a gateway
**self-mounts on the HTTP transport** (`nestrs-http`) via
`HttpEndpointMeta` — inheriting its port, CORS, and TLS — rather than
opening a second server.

poem is the first-class WS engine for nestrs, riding the same HTTP
transport. The crate is intentionally poem-typed at the connection
boundary: the upgrade `Endpoint`, `WebSocketStream`, and
`SocketContext::capture(&poem::Request)` surface poem types.

## What is — and isn't — poem-bound

The crate splits into two layers:

**Poem-bound (the upgrade and the connection loop):**

- `GatewayEndpoint` / `gateway_endpoint` — the `poem::Endpoint` returned
  by `#[gateway]`. Mounts as `HttpEndpointMeta` on `nestrs-http`.
- `SocketContext::capture` — runs on a `poem::Request`, used by
  `nestrs_seaorm::ws::WsDataContext` to seize the post-guard request
  ambient (executor + ability) before the upgrade unwinds.

**Engine-agnostic (connection registry, push surface, dispatch protocol):**

- `WsServer<N>` — connection registry, generic over a zero-sized
  namespace marker `N` (default `Global`). Pure `#[injectable]`, no
  `poem` import. Reachable through any `Transport` impl.
- `Registry` — object-safe push surface (`broadcast_value`,
  `emit_to_value`, `emit_value`, `join`, `leave`). Holds payloads as
  `serde_json::Value` so the trait stays object-safe.
- `WsClient` — per-connection handle a handler receives by declaring
  `&WsClient`. Type-erases the registry's `N` parameter.
- `WsEnvelope` / `WsReply` — the wire-shape and dispatch outcome.
  Independent of any transport.
- `MessageGuard`, `MessageGuardTable` — per-message guards
  (`Err(reason)` ships an error frame). Take `&WsClient` and
  `serde_json::Value` — no `poem::Request`.
- `Gateway` — the per-connection dispatcher trait `#[messages]` impls.
  `dispatch`, `on_connect`, `on_disconnect` take `&WsClient` — no poem.
- `SocketContext` — the seam through which `nestrs-seaorm` re-installs
  ambient state per message. `capture` is poem-typed; `around` is
  engine-agnostic.

## The engine-agnostic seam

The lifecycle contract is shared with HTTP: `nestrs_core::Transport`.
There is no separate "WS transport" today — gateways ride
`HttpTransport`. An alternative engine integration would have two
choices:

- **Mount over its own HTTP engine.** A `nestrs-http-axum` integration
  would expose its own endpoint-meta seam (the axum equivalent of
  `HttpEndpointMeta`); a paired `nestrs-ws-axum` crate would emit
  gateway endpoints against that seam, using axum's WebSocket extractor.
  The engine-agnostic types in this crate (`WsServer<N>`, `Registry`,
  `WsClient`, `WsEnvelope`, `WsReply`, `MessageGuard`, `Gateway`) carry
  over **unchanged** — that work is purely the upgrade adapter and the
  connection loop.
- **Run a standalone WS transport.** An alternative could implement
  `Transport` directly (skipping HTTP self-mount) and contribute it
  through `TransportContribution`. The trade-off is losing the inherited
  port/CORS/TLS.

## What stays shared between engines

- `WsServer<N>`, `Registry`, `WsClient`, `WsEnvelope`, `WsReply`,
  `Gateway`, `MessageGuard`, `MessageGuardTable` — all pure Rust over
  `serde_json::Value` and channels.
- The JSON envelope (`{event, data}`) and the dispatch protocol.
- Per-message guards and the connection-level vs per-message scoping
  rule.
- `SocketContext::around` (engine-agnostic) — only `capture` would
  rewire to the alternative engine's request type.

## What this crate exports

- `WsModule` — registers the default `WsServer<Global>` connection
  registry.
- `WsServer<N>`, `ConnId`, `Global`, `Registry`, `WsClient` — the
  registry + push surface.
- `WsEnvelope`, `WsReply` — the wire types.
- `Gateway`, `GatewayEndpoint`, `gateway_endpoint` — the dispatcher
  trait + the poem-mounted endpoint.
- `MessageGuard`, `MessageGuardTable` — per-message guard trait + the
  per-event dispatch table built by `#[messages]`.
- `SocketContext`, `Captured`, `BoxFuture` — the per-connection ambient
  bridge.
- Re-exports: `pub use poem`,
  `pub use async_trait::async_trait`, `pub use serde_json`,
  `pub use tracing`,
  `pub use nestrs_middleware::{EndpointExt, Guard}`,
  `pub use nestrs_ws_macros::{gateway, messages}`.

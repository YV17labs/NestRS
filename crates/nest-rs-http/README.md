# nest-rs-http

> Part of **[NestRS](https://nestrs.dev)** (alpha). **[Documentation](https://nestrs.dev/http)** · [HTTP](https://nestrs.dev/http/) · [Getting started](https://nestrs.dev/getting-started/)

The poem-backed HTTP transport for [nestrs](https://nestrs.dev). It mounts
`#[controller]` routes, hosts self-mounting endpoints contributed by other
surfaces (GraphQL, MCP, WebSocket gateways), wires the framework's named
middleware (`Guard`, `Filter`, `Interceptor`), and serves the assembled tree
through [poem](https://docs.rs/poem).

poem is nestrs' first-class HTTP engine. The crate is intentionally
poem-typed: `Route`, `Endpoint`, `Request`, `Response`, `Cors`, `TlsConfig`,
the per-handler extractors (`Json`, `Path`, `Query`, …) all surface as-is
to apps and macros. Treat that as a contract, not an accident.

## The engine-agnostic seam

The framework's lifecycle contract for "anything that accepts inbound
requests" is [`nest_rs_core::Transport`]. `HttpTransport` is one implementor.
A community-driven integration over a different engine — for example
`nest-rs-http-axum` over [axum](https://docs.rs/axum), or `nest-rs-http-actix`
over [actix-web](https://docs.rs/actix-web) — writes its own crate against
the **same** seam:

| Layer | Engine-agnostic contract | This crate's poem impl |
|------|-------------------------|------------------------|
| Lifecycle (`configure` + `serve`) | `nest_rs_core::Transport` | `HttpTransport` |
| Module → transport contribution | `nest_rs_core::TransportContribution` | `HttpModule::for_root(...)` |
| Route discovery (per controller) | each engine defines its own controller-meta type | `HttpControllerMeta` |
| Self-mounted sub-endpoints | each engine defines its own endpoint-meta type | `HttpEndpointMeta` |
| Global interceptor discovery | each engine defines its own interceptor-meta type | `HttpInterceptorMeta` |

`Transport` and `TransportContribution` live in `nest-rs-core` and name no
HTTP types — that is where the seam is. Everything in `nest-rs-http` that
mentions `poem::*` is the **poem** implementation of that seam.

## Writing an alternative HTTP engine integration

A hypothetical `nest-rs-http-axum` would:

1. Define its own controller / endpoint / interceptor metadata types
   mirroring `HttpControllerMeta` / `HttpEndpointMeta` /
   `HttpInterceptorMeta`, carrying closures over the axum router type.
2. Ship a corresponding `#[controller_axum] + #[routes_axum]` macro pair
   (separate from `nest-rs-http-macros`) that emits those metadata types
   and serializes the per-handler dispatch against axum's extractor /
   handler model.
3. Implement `nest_rs_core::Transport` over its own router builder and
   contribute it through `TransportContribution`.
4. Provide its own named-middleware traits for the engine
   (`nest-rs-middleware` is poem-typed by design — a separate
   `nest-rs-middleware-axum` would mirror its `Guard` / `Filter` /
   `Interceptor` vocabulary against `axum::extract::Request` and
   `axum::response::Response`).

What stays shared between the two:

- `nest-rs-core` (DI, modules, access graph, transport lifecycle).
- `nest-rs-config` (`NESTRS_HTTP__*` env scheme; the axum integration would
  use `NESTRS_HTTP_AXUM__*` or its own namespace).
- `nest-rs-pipes` (transport-agnostic input pipes — only the per-engine
  *adapter* like `Valid` / `Piped` is engine-bound).
- Every `crates/features/<feature>/` port (services, entities, errors —
  no HTTP types).

What stays poem-only:

- `crates/features/<feature>/http/` adapters (their controllers
  serialize to poem types).
- `nest-rs-openapi`, `nest-rs-ws` (mounts as poem endpoints today),
  `nest-rs-graphql` (mounts `async_graphql_poem::GraphQL`).

An alternative engine would either contribute its own `nest-rs-ws-<engine>`
/ `nest-rs-graphql-<engine>` re-mounts, or coexist with this crate by
running the engines on separate `Transport` instances inside the same app.

## What this crate exports

- `HttpTransport` — the poem-backed `Transport` impl. Built and mounted
  by `HttpModule::for_root(...)`.
- `HttpModule`, `HttpSetup`, `HttpConfig`, `TlsConfig`, `CorsConfig` —
  configuration surface (dual-path: env + pinned).
- `HttpControllerMeta`, `HttpRouteMeta`, `HttpVerb`, `Controller`,
  `SchemaFn`, `schema_of` — discovery metadata for `#[routes]`-generated
  controllers, also consumed by `nest-rs-openapi`.
- `HttpEndpointMeta` — the seam by which GraphQL, MCP, and WebSocket
  gateways self-mount on the HTTP transport.
- `HttpInterceptorMeta` — discovery for global `#[interceptor]`s.
- `RouteResponseShaper`, `ShapedEndpoint`, `shaped` — per-route response
  shaping (used by `nest_rs_authz::http::Authorize`).
- `Ctx<T>`, `Scoped<T>`, `RequestScopeEndpoint`, `Reflector` —
  handler-side helpers (poem-typed).
- `Valid<E>` / `Piped<P, E>` / `IntoInner` — poem adapter for
  `nest-rs-pipes`.
- Re-exports: `pub use poem`, `pub use schemars`,
  `pub use async_trait::async_trait`,
  `pub use nest_rs_middleware::{EndpointExt, Filter, Guard, Interceptor, Next, RequestSnapshot}`,
  `pub use nest_rs_http_macros::{controller, crud, interceptor, routes}`.

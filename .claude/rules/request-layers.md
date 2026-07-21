---
paths:
  - "crates/nest-rs-http/**/*.rs"
  - "crates/nest-rs-http-macros/**/*.rs"
  - "crates/nest-rs-guards/**/*.rs"
  - "crates/nest-rs-pipes/**/*.rs"
  - "crates/nest-rs-filters/**/*.rs"
  - "crates/nest-rs-interceptors/**/*.rs"
  - "crates/nest-rs-exception-filters/**/*.rs"
  - "crates/nest-rs-ws/**/*.rs"
  - "crates/nest-rs-graphql/**/*.rs"
  - "**/controller.rs"
  - "**/resolver.rs"
  - "**/gateway.rs"
---

# Request layers — one pool, exactly once

## Controllers are thin

A handler wires layers, each with one home: **Guard** (gates access,
attaches context), **Pipe** (stateless edge conversion/validation),
**Bind** (id → loaded + authorized entity), **Service** (business + sole
DB gateway), **Interceptor** (cross-cutting, e.g. transaction wrapping).

**Inline conversion, permission checks, or transaction management in a
handler is drift.**

## The invariant

Declaring a layer (guard / pipe / interceptor / filter / exception-filter)
at any scope — **global** (imperative `use_*_global`), **controller** (on
the struct), **handler** (beside the verb) — contributes to ONE pool per
family, deduplicated by `TypeId` through `compose_chain`
(`nest-rs-core/src/layer_chain.rs`, the single dedup logic for all five
families).

The layer executes **exactly once per request**; broadest scope wins;
`#[force_*]` is the re-run opt-in. Scope never multiplies executions — it
chooses the **execution site**, matched to the family's nature:

| Family | Site (global scope) | Site (controller/method) |
|---|---|---|
| Guard | `RouteShaper` (post-routing — reads `#[public]`); `Guarded` self-mount edge; in-band `/graphql` + `/mcp` op-guard | same sites |
| Pipe | `RouteShaper` | `RouteShaper` |
| ExceptionFilter | route site (typed catch, closest to handler) | route site |
| Interceptor | **transport edge** (band 90) — sees 404s, denials, self-mounts; runs *before* auth (no principal/ability/executor) | around the handler, *inside* guards |
| Filter | **transport edge** (band 50) | around the handler, *inside* guards |

Teachable rule: *global = around the whole HTTP process; scoped = around
your handler; either way, once.* `Layer::priority` orders entries
*within* a site, never across sites.

**Per-route inner→outer** (from `#[routes]`): handler → ability shaper →
exception-filter pool → scoped filters → scoped interceptors →
RouteShaper (guard pool → pipe pool) → `#[meta]`/`#[public]` (route data).

**Transport bands** (innermost→outermost): routing → DbContext (−10) →
global filter pool (50) → global interceptor pool (90) → infra
`#[interceptor]` (100).

Same relative nesting at both sites: interceptors outside filters,
exception-filters closest to the handler.

**Two ways to be transport-wide, deliberately:** `use_*_global` = the
**pool** (app-listed, TypeId-deduped against narrower scopes);
`#[interceptor]` = **infra** a module import brings (auto-mounted, off
pool, fixed band — `DbContext`, tracing, timing).

## Guards

A `Guard` borrows the request **mutably** — gates access (returns
`Err(Denial)`), may attach context read back via `nest_rs_http::Ctx<T>`.
**Denials are `Ok(4xx)` responses, never `Err`** — filters don't see
them; global interceptors observe them. Per-handler metadata via
`#[meta(EXPR)]` + `nest_rs_http::Reflector`.

## Fail-secure boot

Specs resolve at `configure`: an unresolvable global spec (provider's
module not imported) **fails boot** naming the type (`HttpBootCheck`) —
never a silent drop. An imperative `HttpTransport::mount(...)` under
active global guards fails boot too (`fail_secure_strict`, default
`true`; `false` downgrades to warn).

Self-mounts declare an `EdgePosture`: `Guarded` (default — WS upgrade)
gets the global chain at its edge; `Exempt` (graphql / mcp / openapi)
gates in-band or is deliberately public.

`/graphql` and `/mcp` stay fail-secure under `Exempt` through their
**fallback operation guard**: with no registered
`GraphqlOperationGuard` / `McpOperationGuard`, the global guard pool runs
in-band per operation (a registered bridge *replaces* it — it runs the
same guards itself, so nothing double-runs). The graphql endpoint's
`Public` data marker is load-bearing: it lets `AuthnGuard` admit
anonymous operations through to resolver gates. `/mcp` carries no such
marker (an unauthenticated tool call is refused) and its *no-pool* tail
is deny-all, so the fallback only ever widens what `use_guards_global`
declared.

## Mapped errors never commit

A route-site `Filter`/`ExceptionFilter` that maps a handler `Err` to a
response tags it `nest_rs_core::MappedError`; `DbContext` rolls back
regardless of the mapped status. (Global filters sit outside `DbContext`
— the rollback already happened.)

## Versioning

URI versioning: `#[controller(version = "1")]` mounts under `/v1`
(`version_path` is the single source of truth).

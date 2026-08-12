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
| Guard | `RouteShaper` (post-routing — reads `#[public]`); `Guarded` self-mount edge; in-band `/graphql` + `/mcp` op-guard | same sites, plus the per-operation chain on `/graphql` and `/mcp`, and the per-message table a gateway freezes at mount |
| Pipe | `RouteShaper` | `RouteShaper`; per argument on graphql/ws/mcp/queue |
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

## The posture is the fifth site, and it is not a pool

`#[authorize(Action, Entity)]` / `#[public]` is **not** a layer: it declares the
operation's access posture, and the impl-half decorator turns it into a class gate
plus a response mask. All four request-carrying edges emit it — `#[routes]` via
the `Authorize<A, E>` shaper, `#[operations]` / `#[messages]` / `#[tools]` via
`nest_rs_authz::<edge>::{authorize, masked_*}` — and on the last three it
is **mandatory: no posture ⇒ compile error**. The grammar is one
`PostureRules` in `nest-rs-codegen`, so the three cannot word the same rule three
ways.

Ordering inside an operation is fixed and load-bearing: **chain → gate → pipes →
call → mask**. The gate precedes the pipes so a caller the gate refuses never pays
for validation, and a validation message never doubles as an existence oracle.

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

**The two in-band transports also run a per-operation chain**, composed once
per site in a shared `SiteChainCell` (`nest-rs-guards/src/dispatch/chain.rs`)
from the provider-scope `#[use_guards]` plus the operation's own. They differ in
where their pool executes (`GlobalScope`): `/graphql` runs it at the resolver
site, `/mcp` at its endpoint guard.

**On `/mcp`, what the edge already ran is *reported*, never assumed.**
`McpOperationGuard::already_ran` names the layers that guard executed, and those
are dropped from every bucket **before** dedup. Both halves are load-bearing: a
guard the edge ran must not run twice, and one it did *not* run must still run
even when the app-wide pool contains it — a registered bridge runs its own two
guards and nothing from the pool. Deleting pooled entries on the assumption the
edge ran them silently drops a `#[use_guards]` the developer wrote, which is a
fail-open; `operation::a_guard_the_edge_did_not_run_still_runs_per_operation`
is the regression test.

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

## Versioning — declared on every edge that has an address, refused on the rest

**Versioning is addressing.** A transport carries `version = "…"` if and only if
it has an address a *client* selects; the answer is settled and closed:

| Decorator | `version` | Because |
|---|---|---|
| `#[controller]` | three strategies, several versions, per route | URL, header, `Accept` |
| `#[gateway]` | yes — `/v1/ws`, through `version_path` | the socket URL is the mount |
| `#[resolver]` | **named compile error** | one schema, one introspection — evolve the field, deprecate the old |
| `#[mcp]` | **named compile error** unless a `name` stands beside it | the endpoint is `#[mcp(path = "/mcp/v1")]`; `version` alone is `serverInfo`'s |
| `#[processor]` | **named compile error** | the queue name is the address; splitting it splits the consumer group |
| `#[scheduled]` / `#[listeners]` | **named compile error** | no caller, no wire |

The refusals are the point, not a leftover: a developer must never have to
discover by experiment whether `version` works on an edge. Same pattern as
`reject_http_only_layers`, and the five sentences are worded **once** in
`nest-rs-codegen` with a trybuild snapshot each. Widening this table is an owner
decision.

`#[controller(version = …)]` / `#[gateway(version = …)]` is the one place a
version is **declared**; `version_path` is the one place it becomes a path.
`version = ["1", "2"]` mounts one controller under each, and `#[version("2")]`
narrows a route out of the others — checked against the controller's list by the
`const fn` `versions_declare`, so a stranger is a compile error at the route
rather than a handler that mounts nowhere.

How a caller **selects** one is deployment config (`NESTRS_HTTP__VERSIONING`):
`uri` (the default — the version is in the path), `header`, or `media_type`. The
last two are a rewrite in front of routing (`nest-rs-http/src/versioning.rs`),
inside the global prefix, so one route table serves all three and the served,
logged and documented paths cannot drift. Under a non-URI strategy the URI form
is a `404` — one way to ask — and a malformed version token is a `400` before it
reaches a path.

**Neutral and fallback are different, and the selector computes which it is.**
The transport hands it three exact lists at boot — the routes that carry a
version (as mounted), the addresses answered without one, and the paths
self-mounted endpoints own — and the precedence between them *is* the behaviour:

| Address | A **stated** version | A **default** version |
|---|---|---|
| a self-mount (`/graphql`, `/mcp`, a gateway) | served as sent | served as sent |
| an unversioned controller route | the version wins | served as sent |
| versioned only | resolves, or `404` if another version serves it | resolves |
| nothing versioned | passed through — the router answers | passed through |

A self-mount owns its path outright and has no version to be rewritten to. A
stated version is the strongest signal a caller sends, so it beats an unversioned
neighbour — otherwise `#[controller(version = …)]` is unreachable at that
address. A default is the weakest: the caller asked for nothing, so nothing moves
under them, which is what stops a versioned `#[controller(path = "/")]` with a
catch-all from swallowing the app.

**Matching is loose on purpose, and only safe because poem decides last.** Every
outcome ends at the router: a match yields a rewritten path it must still
recognise, a non-match passes the request on as written. A false match costs a
`404` the request was heading for anyway; a false non-match once served one
controller's body to a caller who asked for another's. Loose in the direction the
router can correct, never in the direction it cannot see. The matcher therefore
reads every segment form the router parses — `:name`, `<regex>` (one segment, not
a tail), `*rest` (the rest, including nothing), and a literal sharing a segment
with a parameter (`/@:handle`). **Matching on controller *prefixes* is the shape
this replaced**, and it was wrong in both directions at once: `/` matched
nothing, so a root-mounted versioned controller silently served the wrong
version; `/posts` matched a different controller's `/posts/drafts`.

**The wrap is skipped when nothing is versioned.** A non-URI strategy with no
versioned controller can never change an outcome, and the endpoint cost +57% per
request to prove it.

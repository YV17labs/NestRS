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
| Guard | `RouteShaper` (post-routing — reads `#[public]`); `Guarded` self-mount edge; in-band `/graphql` + `/mcp` op-guard; the `_service` / `_entities` gate | same sites, plus the per-operation chain on `/graphql` and `/mcp`, and the per-message table a gateway freezes at mount |
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

**The two federation root fields are the fifth guard site, and they had none.**
`_service` and `_entities` are resolved by async-graphql's own `QueryRoot`,
above the merged root, so the chain `#[operations]` emits inside a body cannot
reach them — `_service` answered a `check_graphql` deny-all pool with the whole
SDL, and `_entities` ran the chain once *per representation*, inside whichever
member the reference matched. A schema `Extension` is the one seam async-graphql
leaves in front of a root field, and the app-wide pool runs there, once per
field. Three facts follow, each load-bearing:

- **The pool is the whole chain at that site**, because a federation field
  belongs to no resolver: no `#[use_guards]` scope to compose, no posture to
  read.
- **`#[entity]` bodies therefore compose everything *but* the pool**
  (`GlobalBucket::Skip`) — it ran at the field, and folding it again would put
  the multiplier on every pooled check in the caller's hands. No other site may
  subtract a bucket without naming where it ran instead.
- **`Guard::check_graphql` takes a `GraphqlOperationContext`, not a `Context`**,
  and that is the same rule as everywhere else: async-graphql hands an extension
  an `ExtensionContext` and offers no public constructor bridging the two, so one
  declaration covering both sites has to be worded over what both can give.
  `operation.context()` returns the real `Context` where one exists and `None` at
  the federation site, which is a refusal a guard can read rather than a
  fabrication it cannot.

**A GraphQL `#[entity]` is a `#[query]` for every one of these**, and it is the
role where that matters most: the router resolves it from a *reference* the
client never wrote (`_entities`), so nothing in the document names it and a
forgotten posture is invisible from the outside. Ungated, every `@key`-ed type
in the schema is readable past every `#[authorize]` in it — hence the same
mandatory posture, worded to name the reason. It is a role and not a modifier:
`_entities` is a `Query`-root field, so combining `#[entity]` with `#[mutation]`
or `#[subscription]` is a compile error naming both.

**Being unnamed is also why it is stricter than a `#[query]` in six places**,
each a compile error, and they are named rather than counted because a count
drifts the day one is added: a `Result` return (the chain is emitted only where a
denial has somewhere to go — a bare-return entity would silently have none, and
unlike a `#[query]` nothing in the document would show it), no `bind = Service`
(its `NOT_FOUND`/`FORBIDDEN` split is an existence oracle on a field addressed by
key), no `#[entity(key = …)]` (the key is inferred from the arguments), no
`#[graphql(…)]` of the method's own (async-graphql reads the *first* one on a
method and the decorator has to emit `#[graphql(entity)]` there, so the
developer's would silently take its place and the method would stop being an
entity), `async`, and at least one argument. Five live in `entity_refusals`; the
`bind` one is refused where the posture is parsed.

**Four boot refusals carry the rest**, because none is expressible at one site:
an `#[entity]` without `GraphqlConfig::federation` (async-graphql serves
`_service`/`_entities` from the keys alone, so the flag would otherwise be a
comment); two claims on one **key shape** — which clash on no field name, so the
duplicate-operation check reads the registry's key shapes instead, within a
resolver as well as across them, one `impl` holding two `#[entity]` methods
being a single registration; two claims whose shapes **overlap**, at either
scope; and an `#[entity]` whose resolved type the registry keys
nothing on — `add_keys` returns silently for anything that is not an object or
an interface, so a list, a scalar or a union registers no key, never joins
`_entities`, and leaves the resolver as code the schema does not mention. That
last one is also what disarmed the `federation` refusal: a schema with no key
looks exactly like a schema with no entity.

The shape and not the type, because several `@key`s per type is Apollo's — but
*disjoint* shapes only. `find_entity` matches a reference by "these key fields
are present", so `id` and `id tenant` both answer `{id, tenant}`, one of the two
bodies is unreachable, and the posture that runs is the other one's. Ordering the
members would fix the dispatch and not the posture, which is why this is a
refusal rather than a sort.

**At both scopes, and the exemption that used to be here is why.** It read
"async-graphql sorts one `#[Object]`'s matchers by key arity, so the specific one
wins there" — upstream sorts on `args.len()`, so a key with a non-key argument
beside it outranks a longer key without one, and at equal counts the stable sort
makes declaration order decide. Neither is something a developer declared. So:
disjoint, or one claim, wherever it is written. Narrower than Apollo, argued
rather than inherited. Overlap is computed on the **top-level** field names —
`"id organization { id }"` selects two, not four — because that is what
`find_entity` matches on.

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
from the app-wide pool, the provider-scope `#[use_guards]` and the operation's
own — one `compose`, the same three buckets and the same `TypeId` dedup as
everywhere else. There is no per-transport switch over where the pool executes,
and there was: `/mcp` excluded it, which left a global `check_mcp` guard
unreachable at the only site that could consult it.

**An `Exempt` endpoint guard checks the request; the site checks the operation.**
`GlobalPoolMcpGuard` / `GlobalPoolOperationGuard` (and a registered bridge) are
handed a `&mut Request` and run `check_http`; the per-operation chain is handed
the operation and runs `check_mcp` / `check_graphql`. **Neither stands in for the
other** — a guard the edge authenticated is not thereby excused its operation
check, and subtracting one from the other can only ever skip a check that was
written to run. `operation::a_pooled_guard_checks_the_request_once_and_the_operation_once`
holds both halves: one `check_http` per request, one `check_mcp` per operation
however many scopes declared the guard.

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
response tags it `nest_rs_http::MappedError`; `DbContext` rolls back
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

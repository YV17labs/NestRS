---
paths:
  - "crates/nest-rs-http/**"
  - "crates/nest-rs-http-macros/**"
  - "crates/nest-rs-ws/**"
  - "crates/nest-rs-ws-macros/**"
  - "crates/nest-rs-graphql/**"
  - "crates/nest-rs-graphql-macros/**"
  - "crates/nest-rs-mcp/**"
  - "crates/nest-rs-mcp-macros/**"
  - "crates/nest-rs-guards/**"
  - "crates/nest-rs-authz/**"
  - "crates/nest-rs-authn/**"
  - "crates/nest-rs-pipes/**"
  - "crates/nest-rs-filters/**"
  - "crates/nest-rs-interceptors/**"
  - "crates/nest-rs-exception-filters/**"
  - "crates/nest-rs-throttler/**"
  - "crates/nest-rs-openapi/**"
---

# Edges — transports, posture and request layers

## A transport aggregates

A transport merges contributions from several providers onto one mount:
controllers into one route table, resolvers into one schema, `#[mcp]` hosts into
one endpoint. **Owning a whole mount is the exception and is argued**, because a
product with two features on one owned mount must fold them into a god-adapter.
Merge a link-time registry filtered by `ReachableProviders` (GraphQL, queue,
schedule, events), or container metadata (MCP: each host attaches `McpHostMeta`,
the first on a path the one `HttpEndpointMeta` that mounts them all).

- **Merging adds one failure: two contributions claiming one addressable name**,
  a boot error naming both owners (a duplicate tool name within an MCP path).
- **The mount's identity belongs to the app**, never to whichever contribution
  registered first. The app declares it once; at most one contribution refines
  it for its own mount; a declaration replaces only what it states, so
  capabilities stay observed from the contributions; a declaration reaching
  nothing, or two on one mount, fails the boot; a mount left at the SDK's
  default identity is a boot `warn`, compared against the SDK's own constructor.
- **WS is the one argued exception.** A gateway owns its path (two on one path
  fail the boot): a socket per feature costs a client a connection and nothing
  structural, and cross-feature fan-out is `WsServer<N>`'s. Two features on one
  socket would be a framework change on this pattern, never a workaround.

## What a new edge owes

The edge vocabulary is closed (`architecture.md`); the form is open and costs
all of this — a missing item is a hole a developer finds at the worst moment.
Review a new edge, or a change to one, against it:

1. **A decorator pair** (`macros.md`).
2. **A mandatory posture per operation** — `#[authorize(Action, Entity)]` or
   `#[public]`; no posture is a compile error with a trybuild snapshot. This is
   what keeps *no authn/authz decision outside a guard* true. The grammar is one
   `PostureRules` in `nest-rs-codegen` where it is the same grammar: `#[tools]`
   and `#[messages]` take it verbatim. `#[operations]` parses its own because only
   GraphQL can synthesise an id argument and an `Authorized<A, E>` proof
   (`bind = Service`, `id_arg`); `#[routes]`' posture is optional, because a
   route may be gated by `#[use_guards]` alone, and it refuses `#[authorize]` on
   `#[sse]` and a posture bound to a handler parameter.
3. **A class gate the posture emits** — `nest_rs_authz::class_gate::<A, E>`,
   one function for every edge and ungated by any edge feature, run on the unit
   right after its guards, so `#[authorize]` cannot mean several things; a
   missing ambient ability fails closed. Off a `#[public]` site it refuses a
   caller no principal backs (`Ability::is_visitor`) whatever the visitor's
   rules grant, on HTTP as elsewhere; a hand-written `Authorize<A, E>` on a
   `#[public]` route reads the visitor's grant, the public-reads pattern. One
   refusal reads one sentence on every edge (RFC 6750 §3.1's for a scope) and
   files one `warn`; an edge renders the `Denial` it carries.
4. **Response masking armed by the posture**, never hand-written in a body,
   chosen by the protocol rather than by symmetry (`data-layer.md`, *Response
   masking*).
5. **Guards at two scopes** plus `#[force_guards]` on a method, which binds
   the guard there and re-runs it, composed once per site and
   deduplicated by `TypeId`; a denial renders through one
   `denial_to_<edge>_error`, so a guard's refusal and a gate's look the same.
   **One denial files one line**, its decider's: a guard that files its own
   says so (`Guard::files_its_denials`), as the framework's authentication,
   ability and throttler guards do, and the edge files the structural
   `guard denied …` on `nest_rs::layers` only for one that does not; a class
   gate files its own. Held by a test per edge.
6. **A view, `<Edge>View` in the edge crate's `view.rs`, and the bound the
   decorators emit for it** (`guard_capability_bounds`: `GuardFor<<Edge>View>`).
   A guard has two entries, `check_http` for an HTTP request and `check` for
   every other edge's unit, both reading a `UnitContext` and both admitting by
   default, so without the bound an empty `impl Guard for X {}` passes
   everything; `GuardFor` makes the author declare that the guard checks this
   edge's units. All four edges assert, at every emitter (HTTP has three:
   `#[controller]`, `#[routes]`, and `#[gateway]` for the upgrade), with a
   trybuild snapshot each.
7. **Per-argument pipes**, run **after** the gate, so a refused caller never pays
   for validation and a validation message is never an existence oracle; a
   rejection is the edge's native error.
8. **A named compile error for every layer family the edge does not run** —
   one row in `nest_rs_codegen::layers`' table, read through `take_layers`,
   never a second refusal: a cell is built, pending (refused as a silent no-op
   until the edge runs the family) or refused with its fact, and the change
   that builds a family flips its cell.
9. **Request scope and a unit of work** — `Scoped<T>`, a parameter the boot
   checks (its provider is inside the access contract, through
   `ResolvesProvider` and the impl half's probe — HTTP's today), and each
   dispatch run through `nest_rs_database::run_unit` over the `dyn UnitOfWork`
   resolved at mount, committing on the edge's own outcome, so commit and
   rollback cannot drift (`data-layer.md`).
10. **A `#[config]` and its `for_root`** (`container.md`).
11. **Error opacity** — an error that is neither the edge's own deliberate
    error, a `nest_rs_core::ToProblem` error where the edge reads one, nor
    framework vocabulary is answered opaquely on every edge
    (`nest_rs_core::OPAQUE_CLIENT_MESSAGE`, the `INTERNAL` code), its chain
    logged once at `error`, as is the chain behind a `5xx` problem — at `warn`
    behind a `503`, the server unable to serve the unit in time, a statement
    Postgres cancelled at its bound among them (core's `Withheld` classes it
    once for every edge); a bare `Problem` withholds nothing. `Problem`, `Code` and `ToProblem` are the
    one client-error contract: a `Problem` answers in the edge's standard
    form (a `code` member, `extensions.code`, `errors.code`, `data.code`),
    never with its detail on a `5xx`, a denial carries the same `code`
    wherever it has no status line, and every code the framework emits is a
    `nest_rs_core::problem::code` constant. The edge decides **by type**
    where it names the handler's error — GraphQL's and WS's wrappers probe
    `ToProblem` before the chain, an MCP operation converts with
    `problem_error` — and a `Problem`, `PipeError` or decode failure its
    chain carries answers itself; HTTP reads no `ToProblem`: it renders an
    error's own `ResponseError` as built and reads the chain of what it
    would otherwise render as text. GraphQL holds the rule at
    compile time: async-graphql's `custom-error-conversion` is on, so a `?`
    cannot turn a foreign error's `Display` into a client message. An
    `Opaque` trait beside the edge's error type says the same at a call
    site; the trait is per edge and only the constant is shared, because the
    trait's output *is* the edge's error type, which is what lets
    `.opaque()?` infer. A handler's error is boxed with
    `nest_rs_core::boxed_error`, never `.into()`, so its chain stays
    readable; every reply built from an error says a decode failure without
    its value (`DecodeError::redact`). Held by each edge's behaviour tests
    and a trybuild snapshot of the refused `?`.
12. **Discovery and its gate** (`container.md`).
13. **Aggregation**, as above.
14. **A mount** — a `Transport` attached with
    `ContainerBuilder::provide_transport`, binding in `listen`, never in
    `serve`, and stating a finite `stop_bound` the kernel holds, or an HTTP
    self-mount declaring its `static` `Handler`, whose posture says how the
    pool meets it.
15. **Its target, unit and operation line** (`observability.md`).
16. **A way down** (below), proved over a real socket — an in-process test
    cannot see a socket left open.
17. **Witnesses** — an `integration` suite over guards, pipes, scope and posture;
    a driver in `nest-rs-testing` if the protocol needs one; an adapter in
    `demo/`; a use site in `nest-rs-macro-hygiene`; its `nestrs generate`
    adapter generator (`cli.md`).

Argued, not gaps: a WS gateway owns its mount, and on WS the connection
carries the ability its upgrade's guards attached (`data-layer.md`,
*Extractors and the carried ability*).

## The way down at an edge (`container.md` states the invariant)

- **HTTP** gives open connections its shutdown window, then closes what is left
  with one `warn` counting them; a handler dropped there files its line
  `cancelled`. The request timeout bounds a handler, never a streaming body,
  which is why the window is the transport's own.
- **What has no end of its own ends at the signal, the way its protocol ends
  one**, after the unit it is answering: an event stream (`OpenEndedBody`) ends
  cleanly so `EventSource` reconnects; a gateway's socket closes `1001 Going
  Away`; a graphql-ws socket completes each subscription, answers what is still
  running, then closes `1001`; an MCP `subscriptions/listen` gets its final
  result. Each files its unit `cancelled`. A socket a hand-built self-mount
  upgraded without declaring it is refused this — the transport cannot see
  inside it — and counted.
- **Work an endpoint runs off its connections** (rmcp's operation tasks,
  DataLoader batches, an upgraded socket) is declared as a `DetachedWork`: told at
  the signal, given the rest of the window, then stopped and settled once for
  every mount together.

## Request layers — one pool, exactly once

**Controllers are thin.** A handler wires layers, each with one home: a guard
gates access and attaches context, a pipe converts and validates at the edge,
`Bind` loads and authorizes an entity, the service holds business logic and is
the sole database gateway, an interceptor carries cross-cutting work such as a
transaction. Inline conversion, permission checks or transaction management in a
handler is drift.

**A layer executes exactly once per request.** Declaring a guard, pipe,
interceptor, filter or exception filter at any scope — global
(`use_*_global`), controller, handler — contributes to one pool per family,
deduplicated by `TypeId` in `compose_chain`, the single dedup for all five. The
broadest scope wins and `#[force_*]` is the re-run opt-in. Scope never multiplies
executions; it chooses the **site**, by the family's nature:

| Family | Global scope runs at | Controller / handler scope runs at |
|---|---|---|
| Guard | the route shaper; a self-mount's request (a gateway's upgrade, `/graphql`, `/mcp`); the federation gate | the same, plus each in-band operation's chain and a gateway's per-message table |
| Pipe | the route shaper; `/graphql` variables (a POST or a graphql-ws operation) and WS `data` at their endpoint | the route shaper; per argument on the other edges |
| Exception filter | the route, closest to the handler | the route |
| Interceptor | the transport edge — sees 404s, denials and self-mounts, before authentication | around the handler, inside the guards |
| Filter | the transport edge | around the handler, inside the guards |

*Global is around the whole HTTP process; scoped is around your handler; once
either way.* `Layer::priority` orders within a site, never across; the transport
bands are constants in `nest_rs_http`'s interceptor module, and the per-route
nesting is stated on `#[routes]`' builder. Both sites nest alike: interceptors
outside filters, exception filters closest to the handler. **Two ways to be
transport-wide**: `use_*_global` is the app-listed pool; `#[interceptor]` is
infrastructure a module import brings (`DbContext`, tracing), off the pool.

- **A denial is an `Ok(4xx)` response, never an `Err`**: filters do not see it,
  global interceptors observe it. **The posture's class gate runs with the
  guards**, after them and before any scoped interceptor or filter, however the
  route spells it: a layer that answers for the handler never answers a caller
  the posture refuses. A guard may attach context (`Ctx<T>`); metadata is
  `#[meta(EXPR)]`, declared on any host or handler into the site's one
  `Handler` — the method's value over its host's — and read through
  `Reflector`, never copied into the request a layer can write.
- **Boot is fail-secure.** An unresolvable global spec fails the boot naming the
  type, at the kernel's wiring step (`provide_wiring`), in every app whatever it
  serves; so does a global guard pool listed out of phase order.
  **No site runs without a layer it declares**: one composed at boot (a route, a
  gateway's upgrade and messages) fails the boot naming the layer and the site,
  one composed on its first dispatch (a GraphQL or MCP operation) refuses that
  unit opaquely and files one `error` naming both. **A surface beyond the
  route table is a self-mount**, the transport's one other mount (it is built
  from `HttpConfig` alone, with no builder), and declares a `static` `Handler`
  with its posture: `Gated` (a WS upgrade, `/mcp`) and `InBand` (`/graphql`,
  its operations gating themselves) get the request stage the transport
  composes in front of the endpoint; `Public` (OpenAPI, health, static files)
  is deliberately open. **The transport composes every self-mount's request
  stage, one function for all** (`request_stage`): the global pool, the
  mount's own guards (a gateway's) and the `EndpointGuards<V, G>` the app's
  modules declare for the in-band units it carries, each guard once and in
  phase order, behind the browser-origin check where the mount owes one — a
  refused origin pays for no guard. An endpoint never runs a guard of its own.
- **The in-band edges also run a per-operation chain**, composed once per site in
  a shared `SiteChainCell` from the app-wide pool, the provider's `#[use_guards]`
  and the operation's own — one `compose`, no per-transport switch over where the
  pool runs. **An in-band endpoint's guards check the request; the site checks
  the operation**: `check_http` once per request, `check` once per operation,
  and neither stands in for the other. What a request's guards attach
  (`cx.attach`) is ambient for every unit the request carries.
- **`/graphql` and `/mcp` stay fail-secure**: the global pool always runs at
  the request, and the `EndpointGuards` a module declares add to it, never
  replace it — each guard once, the declared guards where the pool lists the
  first one they name, else after it; declared by several modules, in import
  order. They are providers, so module-gated by construction; `V` is a
  `CarriedView`, a view some endpoint's request carries (a self-mount says so
  with `HttpEndpointMeta::carries`), and a declaration whose endpoint the app
  does not mount fails the boot. GraphQL's
  `InBand` posture lets `AuthnGuard` admit anonymous operations to resolver
  gates; `/mcp` is `Gated`, and with neither pool nor endpoint guards it is
  deny-all, said once at boot and per refusal. The one opening is
  `McpOptions::posture = Posture::Public`, written in code at one import (a
  second is a contested declaration), and a module declaring guards beside it
  fails the boot
  (`.claude/decisions/bridge-joins-the-pool.md`).
- **A mapped error never commits**: a route-site filter mapping a handler `Err`
  tags it `MappedError`, and `DbContext` rolls back whatever status it maps to.
- **The global site takes no capability bound**, by decision: a global guard
  legitimately serves whichever edges it implements, and the pool reaches each
  operation where the operation exists, at all four edges. The pool dispatches
  on `dyn Guard`, so a pooled guard runs the entries it overrides whatever
  `GuardFor` it declares — a missing declaration never opens an edge (held by a
  test per edge); the declaration matters only where a decorator binds the
  guard. A guard declaring `GuardFor<V>` without overriding the entry `V`'s
  units run is a visible line, not a gap to close
  (`.claude/decisions/check-http-stays-on-guard.md`).
- **Only route mounts (global, controller and method scope composed), the global
  bucket and a self-mount's request stage are phase-validated**
  (`.claude/decisions/in-band-phase-validation.md`); **MCP discovery is gated at
  the transport**, where the specification places it
  (`.claude/decisions/mcp-discovery-at-transport.md`).

**The edge-neutral pipeline is in core** (`.claude/decisions/one-execution-pipeline.md`):
`nest_rs::core::{Interceptor, ExceptionFilter}`, `#[interceptor]` (handler
stage) and `#[interceptor(stage = unit)]`, and one `dispatch` — unit stage,
guard stage and class gate, exception filters (method, host, app), handler
stage (the app's, the modules' in import order, host, method), tail. An edge
moves onto it in its own change, a `Site` per dispatch site, and its row of the
table above goes with it; until then that table is what runs.

## Posture inside an operation

The posture is not a layer: the impl half turns it into a class gate and a mask.
Order is fixed: **chain → gate → pipes → call → mask**. **The chain runs whatever
the operation returns** — the failure channel is the emitted wrapper's, never the
developer's method, so a bare `T` answers through a `Result` like any other
(`.claude/decisions/operation-chain-any-return.md`).

**GraphQL's federation root fields are a guard site of their own.** `_service`
and `_entities` resolve in async-graphql's own root, out of reach of any
`#[operations]` body, so a schema `Extension` runs the app-wide pool there once
per field — the whole chain at that site, since the field belongs to no
resolver. An `#[entity]` body therefore composes everything but the pool, and a
`#[field_resolver]` too, naming where the pool ran (the root field that produced
its parent) — never once per parent row; its resolver's own `#[use_guards]` still
run. No site subtracts a bucket without naming where it ran. A guard's `check`
reads the field through `GraphqlView`, whose `role()` is `Federation` at that
site: a refusal a guard can read, not a fabrication.

**An `#[entity]` is a `#[query]` for posture, and stricter**, because the router
resolves it from a reference no document names, so a forgotten posture is
invisible. It is a role, not a modifier (with `#[mutation]` or `#[subscription]`
it is a compile error), and it refuses `bind = Service` (an existence oracle on a
field addressed by key), `key = …` (inferred from the arguments), a
`#[graphql(…)]` of its own (it would replace the one the decorator emits), and an
empty argument list. The boot refuses an `#[entity]` without
`GraphqlConfig::federation`, one whose type registers no key, and two claims
whose **key shapes** are equal or overlap, within a resolver or across them:
`find_entity` matches by the presence of top-level key fields, so overlapping
shapes make one body unreachable and run the other's posture — narrower than
Apollo, on purpose.

## Versioning is addressing

An edge carries `version` if and only if a client selects it by address; the
table is closed, and each refusal is worded once in `nest-rs-codegen` with a
trybuild snapshot:

| Decorator | `version` | Because |
|---|---|---|
| `#[controller]` | several, per route, three strategies | URL, header, `Accept` |
| `#[gateway]` | yes, through `version_path` | the socket URL is the mount |
| `#[resolver]` | compile error | one schema — evolve the field, deprecate the old |
| `#[mcp]` | compile error unless `name` stands beside it | the endpoint is its `path`; `version` alone is `serverInfo`'s |
| `#[processor]` | compile error | the queue name is the address |
| `#[scheduled]` / `#[listeners]` | compile error | no caller, no wire |

`#[controller(version = …)]` / `#[gateway(version = …)]` is the one place a
version is declared and `version_path` the one place it becomes a path; a
`#[version]` narrowing a route outside the controller's list is a compile error.
Selection is deployment config — `uri` (default), `header`, `media_type` — and
the last two are a rewrite in front of routing, inside the global prefix, so one
route table serves all three and the served, logged and documented paths agree.
Under a non-URI strategy the URI form is a `404`; a malformed token is a `400`.
A self-mount is neutral; a **stated** version beats an unversioned neighbour; a
**default** never moves an address the caller did not version. Matching is loose
only in the direction the router corrects, so it reads every segment form the
router parses. The rewrite is skipped when nothing is versioned.

## Surface decisions

- **HTTP** is activated only by importing `HttpModule` — configured by the
  app's one `for_root`, bare in a feature declaring the dependency; no public
  `.transport(...)`.
- **`#[sse]` is a verb, not an edge.** It collapses to `GET` before the route
  table is built and carries the route's guards, pipes, posture and document; it
  owns only the response. It refuses `#[authorize]`, a shaper parameter
  (`Authorize<A, E>`, `Bind<A, S>`), the response decorators (`#[http_code]`,
  `#[redirect]`, `#[response_header]`) and `#[api(response_content_type)]`.
  Correlation is the streaming body's, not the decorator's (`observability.md`).
  Its connection ceiling lives in `HttpConfig`: SSE is not a module, so it owns no
  namespace, and it takes its peers' reading and `0`-is-off spelling.
- **Pipes** are transport-agnostic, one per file, stateless, never DI providers,
  and bind per argument on all five transports in two forms the orphan rule
  forces: HTTP wraps the extractor (`nest_rs_http::Piped`), the others wrap the
  wire value (`nest_rs_pipes::Piped`, inside `Parameters<…>` on MCP). Global
  pipes register in `nest-rs-pipes`, beside their contract, and run over an
  HTTP body, `/graphql` variables — a POST's, or an operation's on the
  graphql-ws socket, a subscription included — and a WS message's `data`,
  each edge resolving the pool once at mount; MCP and queues have none. One
  entry, `GlobalPipe::transform(cx, ArgumentKind, input)`: the unit's context
  says which site, the kind which input, so an edge adds a kind, never a
  method. Reusable pipes are framework primitives — never define one in an app.
- **WS is not a `Transport`**: an upgrade is an HTTP `GET`, so a gateway
  self-mounts on `HttpTransport` and inherits its port, CORS and TLS. One
  envelope, `{event, data}`. A handler returns at most a `Result` around a
  `Result`-free value, decided by type (`ReplyValue`); a `Result` inside an
  `Option`, a `Vec` or a struct is data. An `Err` becomes a frame through
  `ErrorReport`, so a cause chain is logged whenever the type has one.
- **MCP aggregates** behind one endpoint, since clients point at one URL; one
  host on a path is served verbatim. Guard, tool context and config resolve once
  per path. **A host's `path` is a join key, not a namespace**: written whole,
  defaulting to the `DEFAULT_PATH` constant, never config — a decorator's path is
  code everywhere (only a module owning its whole mount, as GraphQL does, may
  configure one), and `global_prefix` moves the whole surface. A failing
  operation talks to a language model, so it answers through `Opaque`; a
  deliberate `McpError::invalid_params` is returned directly, and a `Problem`
  through `problem_error`. **Identity has two
  owners**: the app declares `McpOptions { server }` — name, version, branding
  and `instructions` — and a host may refine only `name` and `title` for its
  endpoint; `instructions` on `#[mcp]` is a compile error. Identity has no
  environment twin, which is why it travels in `McpOptions`.
- **OpenAPI** self-mounts a document **composed from the route table** (schemas
  through `schemars`, `#[api(...)]` enriches) and an offline Swagger UI that
  loads nothing from elsewhere. The document is 3.1 and states only what the
  framework knows (`.claude/decisions/openapi-document-scope.md`); it is a
  development surface (`.claude/decisions/openapi-docs-exposure.md`). A route
  is deprecated with `#[api(deprecated = "…")]`, never Rust's `#[deprecated]`
  (`.claude/decisions/route-deprecation.md`), and a list links its next page
  (`.claude/decisions/pagination-link-header.md`).
- **The throttler's** in-memory default is an ordinary factory a vendor binding
  supersedes; a hit's net sits below the HTTP request timeout.

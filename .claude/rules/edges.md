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
  a boot error naming both owners (a duplicate tool or prompt name, or one
  resource template read by two hosts, within an MCP path).
- **The mount's identity belongs to the app**, never to whichever contribution
  registered first. Its name, version and description default to the binary's
  (`AppIdentity`, which `#[nest_rs::main]` reads off the manifest); the app
  declares the rest, or overrides them, once; at most one contribution refines
  it for its own mount; a declaration replaces only what it states, so
  capabilities stay observed from the contributions; a declaration reaching
  nothing, or two on one mount, fails the boot; a mount left at the SDK's
  default identity — only where no binary identity was captured — is a boot
  `warn`, compared against the SDK's own constructor.
- **WS is the one argued exception.** A gateway owns its path (two on one path
  fail the boot): a socket per feature costs a client a connection and nothing
  structural, and cross-feature fan-out is `WsServer<N>`'s. Two features on one
  socket would be a framework change on this pattern, never a workaround.

## What a new edge owes

The edge vocabulary is closed (`architecture.md`); the form is open and costs
all of this — a missing item is a hole a developer finds at the worst moment.
Review a new edge, or a change to one, against it. **The layers come from one
contract**: an edge writes what is its own — its units, its view, its tail and
how it renders a failure — and calls core's pipeline for the rest, never a
guard, filter or interceptor loop of its own.

1. **A decorator pair** (`macros.md`).
2. **A `unit.rs` and a view.** Each unit the edge opens is a `Unit` constant
   in the edge crate's `unit.rs` (`nest_rs_core::unit!`, named
   `<edge>.<what>` on the crate's target), so its operation line, a layer's
   `Layer::serves` and `cx.unit()` read one name. What a layer reads of the
   unit is `<Edge>View`, in the edge crate's `view.rs`, implementing
   `UnitView` and reached through `cx.switch_to::<<Edge>View>()`; a layer
   answers or settles the unit through it, never through a slot of its own.
3. **The pipeline, inside the unit of work.** Each unit is one `Site` — its
   `Handler` and what its decorators bound — dispatched through core's
   `dispatch` inside `nest_rs_database::run_unit` (`run_unit_bounded` where
   the edge cuts the body at a deadline), over the `dyn UnitOfWork` resolved
   at mount, committing on the edge's own outcome, so commit and rollback
   cannot drift (`data-layer.md`). Its sites compose at boot, so a layer
   nothing provides fails it, and are held by their container, never a
   `static`; a unit nested in another folds only the app-wide buckets its
   parent did not run (`GlobalLayers`) and says where the others ran.
4. **Its tail and rendering arm.** The tail holds every argument it decoded
   to the rules its type declares — with no wrapper, through
   `nest_rs_codegen`'s one `validate_argument`, an extractor, an `Option`
   and a list answering for the value they hold, a pipe's input checked
   before the pipe runs — then runs the per-argument pipes, all after the
   gate: a refused caller never pays for validation, and a validation
   message is never an existence oracle; a rejection is the edge's native
   error — then the handler and the posture's mask, and writes the answer
   into the view. A failure no exception filter answered is rendered by the
   edge's one rule (*Error opacity*, below) before the unit stage returns,
   so a unit-stage interceptor reads what goes out.
5. **A posture where the guards do not already decide** —
   `#[authorize(Action, Subject)]` or `#[public]`, and `#[public]` means one
   thing on every edge: a caller with no credential is admitted. This is what
   keeps *no authn/authz decision outside a guard* true. **Where an endpoint
   admits a visitor in band, the posture is what refuses one, so it is
   mandatory**: a GraphQL operation stating none is a compile error with a
   trybuild snapshot, and an operation of an MCP endpoint `McpAccess::Public`
   opens refuses the boot, whatever guard stands beside it. **Elsewhere a guard
   covering the unit decides, and the posture is optional**: an HTTP route, a WS
   message under its gateway's `#[use_guards]` at the `Gated` upgrade, the pool
   or a guard of its own, and an MCP operation under a `Gated` endpoint the pool
   or `EndpointGuards` cover; `#[authorize]` adds the gate and the mask. A route
   or a message no guard covers says `#[public]` or refuses the boot, one error
   naming every such unit and each remedy, since an impl half cannot see the
   pool; so does a gateway running no guard at its upgrade with no `#[public]`
   message, whatever guards its messages bind: a connection is no message, and
   its hooks and what the server emits reach any caller.
   **`#[public]` that can open nothing refuses the boot**: on
   WS and MCP, behind a guard its request runs that declares
   `GuardPhase::Authentication` or `Authorization`, and on a `Gated` endpoint
   running no guard, which denies every request; behind `Other` guards alone, a
   rate limit, which identify no one, it stays the opening it says, and on HTTP
   an authentication guard reads it and admits the visitor. No gateway opens
   beneath the pool: its upgrade is `Gated`, so the refusal says to move the guard
   onto the hosts it should cover. `AbilityGuard` reads
   it on every edge: a `#[public]` unit no ability reached takes the visitor's
   rules, as a `#[public]` route does, so a tool of an opened endpoint under the
   pool serves, and a gated unit still fails closed.
   The grammar is one `PostureRules` in `nest-rs-codegen` where it is the same
   grammar: `#[tools]` and `#[messages]` take it verbatim, and `bind = Service`
   on a `#[resource]` through it, reading the id from its template's one
   variable, `Denied`, `Missing` and an id no v7 answering the resource's not
   found with no body run; a message, a tool, a prompt and a listing name no id
   and refuse the key. `#[operations]` and `#[routes]` parse their own because
   they bind a subject by id off their own address — from a synthesised id
   argument (`id_arg` renames it) or from the one parameter the route's path
   names, its controller's included. On all three the subject lands in an
   `Authorized<A, E>` parameter, which carries the row and never the posture:
   either without the other is a compile error, as is an entity written beside
   `bind` other than the service's. `#[routes]` refuses `#[authorize]` naming an
   entity on `#[sse]` — by the subject's type, a stream taking a subject no table
   holds — and a hand-written `Authorize<A, E>` anywhere but beside `#[public]` —
   by spelling at compile time, by its type at boot
   (`.claude/decisions/route-posture-declared.md`).
   An edge whose units have no caller (a job, a tick, an event) declares
   `Posture::System` and runs no guard stage.
6. **A class gate the posture emits** — `nest_rs_authz::class_gate::<A, S>`,
   one function for every edge and ungated by any edge feature, run on the unit
   right after its guards, so `#[authorize]` cannot mean several things; a
   missing ambient ability fails closed. `S` is any subject: an entity, or a
   capability no table holds that `#[subject]` declares, gated the same way —
   its scope challenge included — and never by a hand-written guard
   (`.claude/decisions/subject-no-table-holds.md`). Off a `#[public]` site it refuses a
   caller no principal backs (`Ability::is_visitor`) whatever the visitor's
   rules grant, on HTTP as elsewhere; a hand-written `Authorize<A, E>` on a
   `#[public]` route reads the visitor's grant, the public-reads pattern. One
   refusal reads one sentence on every edge (RFC 6750 §3.1's for a scope) and
   files one `warn`; an edge renders the `Denial` it carries.
7. **Response masking armed by the posture**, never hand-written in a body,
   chosen by the protocol rather than by symmetry (`data-layer.md`, *Response
   masking*), and by the subject's type: an entity's posture masks, a
   `#[subject]`'s arms none, and an HTTP route it gates reads as unmasked.
8. **Guards at two scopes, and their `GuardFor` bound when the unit has a
   caller**, plus `#[force_guards]` on a method, which binds the guard there
   and re-runs it, composed once per site and deduplicated by `TypeId`; a
   denial renders through one `denial_to_<edge>_error`, so a guard's refusal
   and a gate's look the same — on HTTP unless the refusing guard wrote an
   error of its protocol's own into the view (`HttpView::respond`, a token
   endpoint's RFC 6749 §5.2 answer), which the guard stage serves instead,
   held by its tests. **One denial files one line, the edge's**: every edge
   files a guard's denial through `nest_rs_guards`' one function, `guard
   denied …` on the target the guard declares (`Guard::target`,
   `nest_rs::layers` by default), at `error` for a `500` and `warn` otherwise,
   with one field set — the reason the denial names and, when it keeps a
   source, the whole chain from its own text down, never that text alone —
   and no guard files a line for its refusal (a panic keeps its own); a class
   gate files its own. A guard writes
   one decision, `check`, which `check_http` runs unless the guard overrides
   it, which it does only where the request differs from the unit; a
   precondition an earlier guard attached is `cx.require::<T>()?`, a `500`
   naming both — but the principal the guard declares through
   `expected_principal`, which answers by cause: the boot refuses a chain nothing
   in which attaches it (a route's, or a unit's with the request carrying it,
   whose `produced_principal` says who does), so its absence is a caller admitted
   without one, the caller's `401` on every edge, and on a `#[public]` route the
   `401` a credential it set aside earned, as an `Ambient` parameter answers (RFC
   6750 §3). A guard deciding a visitor
   itself says `PrincipalClaim::or_visitor`, its unattached claim a `warn`
   (`.claude/decisions/check-http-stays-on-guard.md`). **`check` has no default**, so no guard admits a unit it
   never decided — one whose decision is the request's alone admits the
   units it carries in one visible `Ok(())` — held by the trait, with a
   trybuild snapshot per in-band edge. Each emitter asserts
   `GuardFor<<Edge>View>` (`guard_capability_bounds`; HTTP has three:
   `#[controller]`, `#[routes]`, and `#[gateway]` for the upgrade), so a guard
   runs only on the units its decision was written for, with a trybuild
   snapshot each. Held by a test per edge.
9. **A named compile error for every layer family the edge does not run** —
   one row in `nest_rs_codegen::__private::layers`' table, read through
   `take_layers`, never a second refusal: a cell is built, pending (refused as a
   silent no-op until the edge runs the family) or refused with its fact, and
   the change that builds a family flips its cell.
10. **Request scope** — `nest_rs_core::Scoped<T>`, one parameter on every edge,
    which each resolves as it resolves `Ambient<T>` (HTTP through its own
    `FromRequest`, the others through a `__private` parameter function) and the
    boot checks (its provider is inside the access contract, through
    `ResolvesProvider` and the impl half's probe, as is the service
    `bind = Service` loads through on every edge taking it), a miss
    answered opaquely with the type on the operator's line.
11. **A `#[config]` and its `for_root`** (`container.md`).
12. **Error opacity** — an error that is neither the edge's own deliberate
    error, a `nest_rs_core::ToProblem` error where the edge reads one, nor
    framework vocabulary is answered opaquely on every edge
    (`nest_rs_core::OPAQUE_CLIENT_MESSAGE`, the `INTERNAL` code; on HTTP its
    class's problem document and no `detail`, as every `5xx` the edge does not
    describe), its chain
    logged once at `error`, as is the chain behind a `5xx` problem — at `warn`
    behind a `503`, the server unable to serve the unit in time, a statement
    Postgres cancelled at its bound among them (core's `Withheld` classes it
    once for every edge); a bare `Problem` withholds nothing. It is filed
    where the unit's answer is final, by the status the client is sent, so a
    failure an exception filter answers `4xx` files nothing. `Problem`, `Code` and `ToProblem` are the
    one client-error contract: a `Problem` answers in the edge's standard
    form (a `code` member, `extensions.code`, `data.code` on WebSocket and
    MCP, its RFC 9457 `type` and `title` beside it), never with its detail
    or its field map on a `5xx`, a denial carries the same `code`
    on every edge (`denial_code`), an opaque answer the `INTERNAL` one through
    one constructor per edge, a problem HTTP builds from a status alone the
    class core names for that status, a decode failure and a pipe's refusal
    the `INVALID_ARGUMENT` one — the class beside the field map, which
    `errors` names on every edge and nothing else does, each key as that
    edge's client spells the field, GraphQL's read off its schema's names,
    never re-cased — and every code the
    framework emits is a `nest_rs_core::code` constant, read off the
    standard that names its class (its module doc;
    `.claude/decisions/every-refusal-carries-its-class.md`). The edge decides **by type**
    where it names the handler's error — every edge's wrapper, HTTP's
    `#[routes]` included, probes `ToProblem` before the chain, and a body
    answering the edge's own error converts one — by `?` on HTTP, every
    `ToProblem` error being a `ResponseError`, with `problem_error` where
    the error type is a library's — so a domain error maps to the wire once,
    held by the compiler on HTTP, and a hand-written `ResponseError` is an
    answer that is no problem (another protocol's body, a challenge); a
    `Problem`, `PipeError` or
    decode failure its chain carries answers itself, HTTP reading the chain
    of what it would otherwise render as text. GraphQL holds the rule at
    compile time: async-graphql's `custom-error-conversion` is on, so a `?`
    cannot turn a foreign error's `Display` into a client message. An
    `Opaque` trait beside the edge's error type says the same at a call
    site; the trait is per edge and only the constant is shared, because the
    trait's output *is* the edge's error type, which is what lets
    `.opaque()?` infer. A handler's error is boxed with
    `nest_rs_core::boxed_error`, never `.into()`, so its chain stays
    readable; every reply built from an error says a decode failure without
    its value (`DecodeError::redact`), where it failed when the edge decoded
    the payload (`DecodeError::located`), and a library's refusal quoting what
    the request sent — a value its variables held, a name or a header only
    the request named — by kind, matched on the locked version's wording, the
    stated exceptions aside (`.claude/decisions/payload-never-quoted.md`).
    Held by each edge's behaviour tests and a trybuild snapshot of the
    refused `?`.
13. **Discovery and its gate** (`container.md`).
14. **Aggregation**, as above.
15. **A mount** — a `Transport` attached with
    `ContainerBuilder::provide_transport`, binding in `listen`, never in
    `serve`, and stating a finite `stop_bound` the kernel holds, or an HTTP
    self-mount declaring its `static` `Handler`, whose posture says how the
    pool meets it.
16. **Its target, unit and operation line** (`observability.md`).
17. **A way down** (below), proved over a real socket — an in-process test
    cannot see a socket left open.
18. **Witnesses** — an `integration` suite over guards, pipes, scope and posture;
    a driver in `nest-rs-testing` if the protocol needs one; an adapter in
    `demo/`; a use site in `nest-rs-macro-hygiene` per built layer cell; its
    `nestrs generate` adapter generator (`cli.md`).

Argued, not gaps: a WS gateway owns its mount, and on WS the connection
carries the ability its upgrade's guards attached (`data-layer.md`,
*Extractors and the carried ability*). `events` has no adapter in `demo/`,
whose product has no reaction to keep and whose one subscription reads the bus:
its witnesses are `nest-rs-events`' integration suite and `nestrs g events`,
which the scaffold suite compiles, and the adapter lands with a reaction the
demo needs.

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
gates access and attaches context, a pipe converts at the edge, which holds
every argument to the rules its type declares,
`#[authorize(A, bind = S)]` loads and authorizes an entity, the service holds business logic and is
the sole database gateway, an interceptor carries cross-cutting work such as a
transaction. Inline conversion, permission checks or transaction management in a
handler is drift.

**A layer executes exactly once per unit.** Declaring a guard, pipe,
interceptor or exception filter at any scope — global (the root module's
`Global*` declaration), controller, handler — contributes to one pool per
family, deduplicated by
`TypeId` in `compose_chain`, the single dedup for all four. The broadest scope
wins and `#[force_guards]` is the re-run opt-in. Scope never multiplies
executions; it chooses the **site**, by the family's nature. Every family at
every unit, global and scoped together, outermost first:

| Unit | Unit stage | Guards, then the class gate | Exception filters | Handler-stage interceptors | Pipes |
|---|---|---|---|---|---|
| HTTP route | runs | pool, controller, method (`check_http`) | method, controller, app | app, modules, controller, method | the pool over the body, `#[use_pipes]`, per argument |
| HTTP self-mount's request | runs | the request stage: pool, the mount's guards, `EndpointGuards` | app | — | `/graphql`'s variables, a POST's or a graphql-ws operation's |
| HTTP fallback (`404`, `405`) | runs | — | app | — | — |
| GraphQL root field | — (its request ran it) | pool, resolver, field (`check`) | field, resolver, app | app, modules, resolver, field | per argument |
| `#[field_resolver]` call | — | resolver's, its own | its own | its own | per argument |
| WS message | runs | pool, gateway, message (`check`) | message, gateway, app | app, modules, gateway, message | the pool over `data`, `#[use_pipes]`, per argument |
| WS connection hook | — | — (the upgrade's) | pending | pending | — |
| MCP request (`mcp.operation`) | runs | — (the `/mcp` request's) | — | — | — |
| its `#[tool]`, `#[prompt]`, `#[resource]` or `#[resources]` | — | pool, host, operation (`check`) | operation, host, app | app, modules, host, operation | per argument, inside `Parameters` (none on a resource role) |
| Job attempt, tick, listener call | runs | — (no caller) | method, host, app | app, modules, host, method | per argument |

Every unit is one `Site` dispatched through core's pipeline inside `run_unit`
(`.claude/decisions/one-execution-pipeline.md`), its stages in the table's
order and the tail last; the fallback opens no unit of work. A route a
controller registers on `HttpRoutes` by hand is a route all the same, on one
site for every such route: the app's guard pool, filters and interceptors,
inside a unit of work. The view holds the request for the whole unit, so a
layer reading it after the tail reads its head whole.
`Layer::priority` orders within a stage's bucket, never across. **Two ways to
be app-wide, both at the handler stage**: `GlobalInterceptors` (the module
the app boots) and `#[interceptor]` (a module it imports); only `#[interceptor(stage = unit)]` runs
around the guards — `Server-Timing`, the RFC 9728 challenge — and writing it is
the visible opening.

- **A denial is answered by the guard stage**: exception filters do not see
  it, unit-stage interceptors observe it. **The posture's class gate runs with
  the guards**, after them and before any handler-stage interceptor or
  exception filter, however the route spells it: a layer that answers for the
  handler never answers a caller the posture refuses. A guard may attach context, which a handler on
  every edge reads as `Ambient<T>` (`nest_rs::core::Ambient`): a route by its type, the in-band impl halves
  by its spelling, left out of the protocol's arguments, an absent value answered as *Request
  scope*'s miss is; metadata is
  `#[meta(EXPR)]`, declared on any host or handler into the site's one
  `Handler` — the method's value over its host's — and read through
  `Reflector`, never copied into the request a layer can write.
- **Boot is fail-secure.** A global layer its declaring module does not
  reach fails the access graph, and a pool declared below the module the app
  boots, twice, or (for guards) out of phase order fails the kernel's wiring
  step naming it, in every app whatever it serves.
  **No site runs without a layer it declares**: one composed at boot (a route, a
  gateway's upgrade and messages, a GraphQL field or an MCP operation `#[tools]` declares at
  the wiring step) fails the boot naming the layer and the site, one composed
  on its first dispatch (a GraphQL field or an MCP operation of a container
  built by hand) refuses that unit opaquely and files one `error` naming both.
  **A surface beyond the
  route table is a self-mount**, the transport's one other mount (it is built
  from `HttpConfig` alone, with no builder), and declares a `static` `Handler`
  with its posture: `Gated` (a WS upgrade, `/mcp`) and `InBand` (`/graphql`,
  its operations gating themselves) get the request stage the transport
  composes in front of the endpoint; `Public` (OpenAPI, health, static files)
  is deliberately open; a `Gated` one carrying no unit and running no guard
  refuses the boot, but a gateway's, whose messages answer for it (item 5). **The transport composes every self-mount's request
  stage, one function for all** (`request_stage`): the global pool, the
  mount's own guards (a gateway's) and the `EndpointGuards<V, G>` the app's
  modules declare for the in-band units it carries, each guard once and in
  phase order, behind the browser-origin check where the mount owes one — a
  refused origin pays for no guard. An endpoint never runs a guard of its own.
- **The in-band edges also run a per-operation chain**, composed once per site
  from the app-wide pool, the provider's `#[use_guards]` and the operation's
  own — one `compose`, no per-transport switch over where the pool runs: a
  GraphQL field's or a declared MCP operation's `Site` guard stage. **An in-band
  endpoint's guards check the request; the site checks the operation**:
  `check_http` once per request, `check` once per operation — a pooled guard
  writing only `check` runs it at both, since `check_http` defaults to it, and
  one whose decision is the operation's alone admits the request in
  `check_http`. What a request's guards attach (`cx.attach`) is ambient for
  every unit the request carries.
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
  `McpModule::for_root(McpAccess::Public)`, written in code at one import —
  `McpAccess` has that variant alone, gated being the bare import no line
  spells, so no other posture compiles —
  (a second is a contested declaration), and a module declaring guards beside it
  fails the boot
  (`.claude/decisions/bridge-joins-the-pool.md`).
- **A failed unit never commits, answered or not**: a handler's `Err` or a
  `4xx`/`5xx` it answered fails the unit, an exception filter or interceptor
  that answers it changes only what the client reads, and the unit of work
  rolls back whatever status went out.
- **The global site takes no capability bound**, by decision: a global guard
  legitimately serves whichever edges it implements, and the pool reaches each
  operation where the operation exists, at all four edges. The pool dispatches
  on `dyn Guard`, so a pooled guard runs its entries whatever `GuardFor` it
  declares — a missing declaration never opens an edge (held by a test per
  edge); the declaration matters only where a decorator binds the guard. A
  guard declaring `GuardFor<HttpView>` and writing only `check` refuses with
  it, and none admits by default
  (`.claude/decisions/check-http-stays-on-guard.md`).
- **Only route mounts (global, controller and method scope composed), the global
  bucket and a self-mount's request stage are phase-validated**
  (`.claude/decisions/in-band-phase-validation.md`); **MCP discovery is gated at
  the transport**, where the specification places it
  (`.claude/decisions/mcp-discovery-at-transport.md`).
- **A document generator reads the statuses the composed chain declares**
  (`Guard::denial_statuses`, through the site's guard stage), never a guard's
  name, so an alias, a wrapper or the global pool is listed as it runs; held
  by the OpenAPI suite.

**The edge-neutral pipeline is in core** (`.claude/decisions/one-execution-pipeline.md`):
`nest_rs::core::{Interceptor, ExceptionFilter}`, `#[interceptor]` (handler
stage) and `#[interceptor(stage = unit)]`, and one `dispatch`. HTTP, a GraphQL
field, a WS message, an MCP operation, a job's attempt, a tick and a listener
call run on it. A WS connection hook is a unit with no site yet: it carries
what the upgrade attached and runs no layer.

**A GraphQL root field is a unit nested in its `/graphql` request**
(`graphql.field`, inside its operation's unit of work): its site folds the
app's exception filters and handler-stage interceptors, never the unit stage
the request ran. Each family subtracts the same bucket at the same places,
and names where it ran: a `#[field_resolver]` — no unit, once per parent row —
runs its resolver's and its own guards, and only the interceptors and
exception filters declared on it, none of the app's nor its resolver's; an
`#[entity]` folds no pool where the federation site ran it; a
`#[subscription]`'s layers run at its opening, never per item. A
`#[field_resolver]` is grafted onto its parent at compile time, so the schema
build refuses a served one whose resolver the app does not reach, by the parent's
Rust type, since a binary links objects of one GraphQL name it does not all
serve. Every field's
site is composed at the wiring step, so a layer it declares that does not
resolve, or two `#[meta]` values of one type, refuse the boot. A field's
`Ability` is the unit's — the one its class gate checked — and its mask and
`bind` read it there, never off the GraphQL context.

**Every MCP request is one `mcp.operation` unit**, run on rmcp's task inside
the `/mcp` request's context — its scope, its trace, what its guards and layers
made ambient — and a unit of work of its own: its own site runs the unit stage
alone, once around whatever the request reaches, rmcp's decode and routing
included, a `tools/call` naming no declared tool among it. A `#[tool]`, a
`#[prompt]`, a `#[resource]` or the `#[resources]` listing `#[tools]` declares
is a site inside it — a resource's named by its template, never by the values
its URI carries:
its guards' `check`, its class gate, the exception filters and the handler
stage, never a second unit stage; `subscriptions/listen` and the
notifications run none. Every operation's site is composed at the wiring step,
so a layer it declares that does not resolve, or two `#[meta]` values of one
type, refuse the boot. A generic `#[mcp]` struct or `#[tools]` impl is
refused: its operations' sites are `static`s no parameter reaches. A host
writing its `ServerHandler` by hand has no site: a layer or a `#[meta]` beside
its `#[mcp]` would reach nothing, and is a compile error, so its endpoint is
guarded by the pool and the `EndpointGuards` alone.

## Posture inside an operation

The posture is not a layer: the impl half turns it into a class gate and a mask.
Order is fixed: **chain → gate → filters, interceptors → pipes → call →
mask**. **The chain runs whatever
the operation returns** — the failure channel is the emitted wrapper's, never the
developer's method, so a bare `T` answers through a `Result` like any other
(`.claude/decisions/operation-chain-any-return.md`).

**GraphQL's federation root fields are sites of their own.** `_service` and
`_entities` resolve in async-graphql's own root, out of reach of any
`#[operations]` body, so a schema `Extension` — a subgraph's, and any
schema's under a guard pool, the condition an `#[entity]`'s site reads to skip
it — dispatches each through its own `Site`, a `graphql.field` unit of role `federation`: the
app-wide pool its whole guard stage, since the field belongs to no resolver,
the app's filters and interceptors around it. An `#[entity]` body therefore composes everything but the pool, and a
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
empty argument list; every other role refuses async-graphql's own
`#[graphql(entity)]`, which would key a type with none of this. The boot refuses
a keyed schema without `GraphqlConfig::federation` — an `#[entity]`, or an object
a field returns carrying `#[graphql(entity)]`, since async-graphql serves the
federation fields from a key alone — an `#[entity]` whose type registers no
key, and two claims
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
Under a non-URI strategy the URI form is a `404`; a malformed token is a `400`;
both are answered by the fallback's site, as what the router answers itself.
A self-mount is neutral; a **stated** version beats an unversioned neighbour; a
**default** never moves an address the caller did not version. Matching is loose
only in the direction the router corrects, so it reads every segment form the
router parses. The rewrite is skipped when nothing is versioned.

## Surface decisions

- **A transport is activated by the module that serves on it**: every edge
  adapter imports its edge's module bare — `HttpModule` for HTTP and
  WebSocket, plus `WsModule`, `GraphqlModule`, `McpModule`, `QueueModule` or
  `ScheduleModule`, so the edge's config is read — and every framework module
  mounting on HTTP imports `HttpModule` bare, so an app imports what it serves
  and nothing more; events' bus is imported by the port that emits.
  `WsModule`, the registry a port may import to push, attaches no transport.
  The root names a transport only to pin it (`for_root(cfg)`),
  and a backend binding (`RedisQueueModule`) where it chooses the backend; no
  public `.transport(...)`. A host the app reaches whose edge's transport no
  module attaches is named at run (`container.md`, *Discovery and its gate*;
  `.claude/decisions/adapter-imports-its-transport.md`).
- **`#[sse]` is a verb, not an edge.** It collapses to `GET` before the route
  table is built and carries the route's guards, pipes, posture and document; it
  owns only the response. It refuses `#[authorize]` on an entity (a
  `#[subject]`'s gates it), a shaper parameter (`Authorize<A, E>`), the
  response decorators (`#[http_code]`, `#[redirect]`, `#[response_header]`)
  and `#[api(response_content_type)]`.
  Correlation is the streaming body's, not the decorator's (`observability.md`).
  Its connection ceiling lives in `HttpConfig`: SSE is not a module, so it owns no
  namespace, and it takes its peers' reading and `0`-is-off spelling.
- **Pipes** are transport-agnostic, one per file, stateless, never DI providers,
  and bind per argument on all five transports through one carrier,
  `nest_rs::pipes::Piped<P, S>`, over the `Source` its input is read from: an
  HTTP extractor (`Piped<P, Json<T>>`), or the value the edge decodes itself,
  the default (`Piped<P>`, inside `Parameters<…>` on MCP); `Piped(x)` binds the
  output on every edge. `Piped` is no HTTP extractor: `#[routes]` reads it by
  its spelling, so one behind an alias fails to compile rather than skip its
  source's rules. Validation is no pipe and takes no wrapper: the edge
  runs it (item 4), and a hand-written check converts `validator`'s errors
  with `PipeError::from`, so it answers as the edge does. Global
  pipes register in `nest-rs-pipes`, beside their contract, and run over an
  HTTP body, `/graphql` variables — a POST's, or an operation's on the
  graphql-ws socket, a subscription included — and a WS message's `data`,
  each edge resolving the pool once at mount; MCP and queues have none. One
  entry, `GlobalPipe::transform(cx, ArgumentKind, input)`: the unit's context
  says which site, the kind which input, so an edge adds a kind, never a
  method. `input` is the raw JSON, a `serde_json::Value`, the one passthrough
  the project map's correctness list admits: nothing is decoded yet, and the
  handler still receives its argument parsed at the edge
  (`.claude/decisions/one-execution-pipeline.md`). Reusable pipes are
  framework primitives — never define one in an app.
- **WS is not a `Transport`**: an upgrade is an HTTP `GET`, so a gateway
  self-mounts on `HttpTransport` and inherits its port, CORS and TLS. One
  envelope, `{event, data}`. A handler returns at most a `Result` around a
  `Result`-free value, decided by type (`ReplyValue`), which is what the mask
  reads too; a `Result` inside an `Option`, a `Vec` or a struct is data. An `Err` becomes a frame through
  `ErrorReport`, by its type, so the chain a frame withholds is read whenever
  the type has one, and filed as *Error opacity* says.
- **MCP aggregates** behind one endpoint, since clients point at one URL; one
  host on a path is served verbatim, but for a `serverInfo` nobody named,
  which reports the binary's. Endpoint guards, posture and config
  resolve once per path. **A host's `path` is a join key, not a namespace**: written whole,
  defaulting to the `DEFAULT_PATH` constant, never config — a decorator's path is
  code everywhere (only a module owning its whole mount, as GraphQL does, may
  configure one), and `global_prefix` moves the whole surface. A failing
  operation talks to a language model, so its error is rendered by its type:
  its own `McpError` as built, a `ToProblem` error the problem it says, any
  other opaquely, its chain filed — and an exception filter may answer any of
  them with a problem; `Opaque` says the same at a call site. **Identity has two
  owners** over the binary's name and version: the app declares an
  `McpIdentity` on `McpModule::for_root` — branding and `instructions`, and a
  name or version only to override the binary's — and a host may refine only
  `name` and `title` for its endpoint; `instructions` on `#[mcp]` is a compile
  error. Identity has no environment twin, which is why it travels in
  `for_root`'s value, before the config (`container.md`, *`for_root`*).
- **OpenAPI** self-mounts a document **composed from the route table** (schemas
  through `schemars`, `#[api(...)]` enriches) and an offline Swagger UI that
  loads nothing from elsewhere. The document is 3.1 and states only what the
  framework knows (`.claude/decisions/openapi-document-scope.md`); it is a
  development surface (`.claude/decisions/openapi-docs-exposure.md`). A route
  is deprecated with `#[api(deprecated = "…")]`, never Rust's `#[deprecated]`
  (`.claude/decisions/route-deprecation.md`), and a list links its next page
  (`.claude/decisions/pagination-link-header.md`).
- **A page cursor is parsed at the edge.** An `after` no page issued is the
  edge's client error wherever one is read — a `#[crud]` list's `400` or field
  error, a relation's field error — never the first page; an empty one is the
  first page. Held by `nest-rs-seaorm`'s in-process suite, one test per edge.
- **The throttler's** in-memory default is an ordinary factory a vendor binding
  supersedes; a hit's net sits below the HTTP request timeout.

# CLAUDE.md — nestrs

For an LLM picking up this repository. The codebase tells you what *is*; this
file tells you what was **decided** and what must be **respected**. It does
not re-document the code — crate layout, macro signatures, dependency
versions, and mechanics live in the code, which is authoritative.

Committed to a public repository. No machine-local paths, no private
references.

## What this project is

nestrs is an opinionated Rust framework whose central thesis is **the
developer writes business logic; the framework carries the rest**. The
cross-cutting, error-prone concerns — **security (authn, authz, row-level
filtering), transactions, input conversion/validation, discovery,
lifecycle** — must be *transparent*. A feature that forces the developer to
hand-manage any of them is a defect in the framework, not the app's problem.

The leverage is **procedural macros**: decorators that keep application
code as declarative in Rust as decorators make it in TypeScript. Crates
under `crates/nestrs-*` provide the framework building blocks (IoC
container, module trait, decorator macros); `crates/features/` holds the
product's vertical slices, each a port (the feature root) plus one
adapter sub-folder per transport (`http/`, `graphql/`, `ws/`, `queue/`,
`mcp/`); binaries under `apps/<name>/` are `main.rs` + `module.rs`
composing the edge modules they serve.

Describe features by what they do in nestrs, on their own terms — the
decorators and folder layout are the vocabulary.

## Rule priority — Rust first, conventions second

Every change must satisfy **both**, in this order. When they conflict,
**Rust wins** — adapt the convention, do not bend idiomatic Rust or the
type system.

1. **Rust (non-negotiable).** Idiomatic, reviewable Rust: orphan/coherence
   rules, explicit errors (`thiserror` in libraries — no silent failure,
   no swallowed `DbErr` in loaders), no `unwrap()` on production paths,
   honest APIs (`Type::new(deps)` when tests need it), `Result` propagated
   to the transport boundary. Proc-macro `impl` blocks (`#[dataloader]`,
   `#[hooks]`, trait impls) are normal — not an excuse to hide errors or
   bypass `Repo` except where this file names a deliberate exception
   (e.g. shutdown hooks).
2. **Conventions (second).** Module/feature folders, decorator names
   (`#[module]`, `#[controller]`, `#[resolver]`, `#[field]`,
   `#[dataloader]`), thin handlers, one `service.rs` per feature.
   Conventions explain *where* code lives; Rust explains *how* it is
   expressed.

## Where nestrs departs from convention

Deliberate departures. Do not "fix" them back to the conventional style.

| Common habit | nestrs decision | Why |
|---|---|---|
| One umbrella module re-exporting every edge | A feature ships `UsersModule` (the port — entity, service at the feature root) + one `Users<Edge>Module` per transport; an app lists the edges it serves | Imports reflect what the binary actually serves |
| Split a feature's service into many topic files | One `service.rs` holding the whole `UsersService` is fine — do not fragment for aesthetics | No premature decomposition |
| Return `[]` when the DB fails | **Forbidden** — batch/loader methods return `Result` and surface the error | Silent failure violates the Rust-first rule |
| Per-feature error enum for plumbing failures (DB, validation, auth, OAuth) | The framework ships those errors with their HTTP mapping already wired: `nest_rs_seaorm::ServiceError` (DB + validation, mapping behind the crate's `http` feature so a worker does not link poem), `nest_rs_authn::AuthError` / `CredentialError` / `TokenError`. A feature **never** redefines them | A feature only writes its own error type when the failure is genuinely domain-specific (a wire contract its consumers read, a security-critical opaque variant) |
| Re-exporting a service via an `exports` list | `pub trait` + module-private impl, injected as `Arc<dyn Trait>` | Rust visibility is the encapsulation primitive; a list is redundant |
| Per-method transaction decorator | Ambient `task_local!` executor wraps mutating handlers | One concern, no per-method ceremony |
| Per-module sub-container | Single flat container | Simpler; orphan rules already prevent accidental coupling |
| Manual per-endpoint response serialization | `Ability::mask` runs automatically after every handler | Forgetting to redact a field is structurally hard |
| Listing every controller/provider | Inventory-based discovery for resolvers, cron jobs, processors, hooks | The list in a module = the things decorated in it |
| Class-based DI with reflection metadata | Type-id based DI with `#[inject]` fields | Rust has no reflection; types are the source of truth |
| Implicit access check at runtime injection | Compile-time + boot-time access graph (`crates/nest-rs-core/src/access.rs`) | Boot fails with a clear graph error instead of a deep "Cannot resolve" at first request |
| `nest generate` scaffolding | None — copy the reference feature | A manual copy forces reading the pattern once |

## North Star — what "good" looks like

DX targets, not performance promises (Rust performance is the default;
promising it is unserious). The framework's value is in what the
developer does *not* write.

- **A new CRUD feature costs ≤ 60 lines** of Rust in
  `crates/features/<feature>/` (entity + service + controller + resolver
  + per-edge module). When that stops being true, the framework is missing
  leverage — open an issue rather than write the same boilerplate twice.
- **Adding a feature = copying `crates/features/src/users/`.** When the
  copy is no longer enough, fix the exemplar; do not invent a second
  pattern.
- **Security is wired by composition, not by ceremony.** Importing
  `DatabaseModule` + an `Authz<Edge>Module` activates row-level filtering
  on every `Repo` read, transaction scope on every mutating handler, and
  response masking on every returned `#[expose]` body. A handler does not
  opt-in to these — it opts out by not importing the modules. Guards
  (`AuthGuard`, `AppAbilityGuard`) still bind explicitly per route or
  controller, because a route's principal source is a policy decision,
  not a default.
- **A new decorator is a defect if it adds > 0.5 s of incremental compile
  time per use site.** Decorators are leverage; the cost is paid every
  save. Measure before merging.
- **Zero `unwrap` / `expect` in framework hot paths.** Tests and one-shot
  bootstraps may use them; the request/job path may not.
- **One way to do a thing.** Two decorators that solve the same problem
  are a source of pain. Deprecate one before adding a second.

## Monorepo layout — apps, features, framework

Three homes, one mandate each. The dividing rule: a file lives in
`crates/features/` when *any other app could reuse it*; it lives in
`apps/<x>/` only when *this app's exposure decides something the feature
cannot generalize*.

- **`crates/nestrs-*` — the framework.** Generic, product-agnostic
  mechanism (container, macros, transports, the authn/authz *machinery*).
  It never names a concrete `Claims`, entity, or policy — it is generic
  *over* them (`JwtStrategy<C>`, `AbilityGuard<F>`).
- **`crates/features` — the product's feature library.** Every feature is
  a folder under `crates/features/src/`. **Hexagonal architecture applied
  per vertical slice**: the **port** lives at the feature root
  (`entity.rs`, `service.rs`, `dto.rs`, `error.rs`, `module.rs`); each
  **adapter** is a sub-folder per transport (`http/`, `graphql/`, `ws/`,
  `queue/`, `mcp/`) carrying its own `module.rs`. The port being the
  default — not a sub-folder named `core/` — is deliberate: the things
  every transport needs sit at the obvious place.
- **`apps/<name>` — pure composition.** `main.rs` + `module.rs` (and
  nothing else by default): `module.rs` is a `#[module(imports = [...])]`
  listing the edge modules this binary serves. A feature folder under
  `apps/<x>/` is the **exception**, justified only when the app has an
  endpoint that **could not live in `features`** (a glue handler over
  several features, a one-off route specific to this deployment).

**Port + adapters in practice** (using `users/`):

| Path | Contents | Module struct |
|------|----------|---------------|
| `users/` (root) | `entity.rs`, `service.rs`, `dto.rs`, `error.rs`, `module.rs` | `UsersModule` (the port) |
| `users/http/` | `controller.rs`, `error.rs` (HTTP error mapping) | `UsersHttpModule` (imports `UsersModule`) |
| `users/graphql/` | `resolver.rs` (field + root) | `UsersGraphqlModule` (imports `UsersModule`) |
| `users/ws/` | `gateway.rs` | `UsersWsModule` (imports `UsersModule` + `WsModule`) |
| `users/queue/` | `processor.rs` | `UsersQueueModule` (imports `UsersModule`) |
| `users/mcp/` | `tool.rs` | `UsersMcpModule` (imports `UsersModule`) |

An app that needs only HTTP imports `UsersModule + UsersHttpModule`. A
worker imports `UsersModule + UsersQueueModule`. `UsersModule` provides
the service; importing only it gets the data layer without mounting any
endpoint. Each adapter imports `UsersModule` explicitly —
**composition, not inheritance** (the access graph carries the
dependency, not a class hierarchy).

Orphan rules still bind co-location, but inside the same crate. A
`#[field]` resolver expands to `#[ComplexObject] impl Entity`; the entity
is in `users/entity.rs`, the resolver in `users/graphql/resolver.rs`
— both in the `features` crate, so the impl is local. Field and root
resolvers merge into a single `UsersResolver` in `users/graphql/`.

**One `#[module]` per folder.** The DI module file is **always**
`module.rs`, and a `module.rs` defines **exactly one** `#[module]`
struct. Multiple modules per feature ⇒ multiple folders, never multiple
`#[module]`s in one file. Adapter folders are pluralized after the *role*
when more than one variant lives there (`pipes/` for concrete pipe
impls, `strategies/` for concrete strategy impls); the trait file stays
at the parent (`pipe.rs`, `strategy.rs`). **No `*_module.rs` ever** —
the role goes in the folder name, the file is `module.rs`.

## Macros and the container

### Reach for macros first

`#[injectable]`, `#[module]`, `#[controller]`, `#[routes]`, the per-verb
attributes and their siblings are how application code stays
declarative. When you wire a service, a feature module, or an endpoint,
use them. When a pattern recurs and no macro covers it, **write a new
decorator macro** rather than hand-roll the boilerplate (the threshold
is in *When (not) to write a new decorator* below).

A `proc-macro` crate can export only macros, so each decorator lives in
a companion `*-macros` crate re-exported by its home crate (e.g.
`#[controller]` in `nest-rs-http-macros`, re-exported so apps write
`nest_rs_http::controller`). Shared token helpers go in `nest-rs-codegen`.
A `*-macros` crate **must not** depend on its surface crate — it emits
absolute-path tokens resolved at the call site, so there is no cycle.
Macro-generated code always uses absolute paths (`::nest_rs_core::*`,
`::poem::*`, `::std::sync::Arc`); never rely on call-site scope.

### Controllers are thin

A handler holds no business logic and no ad-hoc conversion — it wires
the layers, each with one home:

- a **Guard** decides access and attaches request context (caller, tenant);
- a **Pipe** converts/validates an input at the edge (stateless, no container);
- a **Bind** extractor resolves an id to its loaded, authorized entity
  (DB-backed edge conversion — what a Pipe can't do);
- a **Service** holds the business logic and is the entity's single DB gateway;
- an **Interceptor** carries cross-cutting work (e.g. wrapping a handler
  in a transaction).

Inline conversion, permission checks, or transaction management in a
handler is drift — push it into the matching layer.

### The DI container is internal

The Rust DI ecosystem was surveyed; none met our maintenance bar. The
container in `crates/nest-rs-core` is ours and stays ours. **Do not
propose adopting an external DI crate.** If ergonomics fall short,
extend ours.

### Composition model

- **`App::builder().build().await` runs four phases** independent of
  call order: *seeds* (runtime values a `main` computes), *collect*
  (each module queues the async factories its import tree owns),
  *factories* (every queued factory is awaited — a seed wins over a
  module factory of the same type), *register* (providers built last,
  injecting seeds + factory outputs). `main` holds only
  `App::builder().module::<AppModule>()` (+ transports); everything a
  module needs is declared *in* the module tree. Sync apps keep
  `App::new`.
- **Providers are singletons** unless `#[injectable(scope = request)]`
  — a per-request factory, built once per request, resolving its deps
  from the singleton root. **One level deep**: request-scoped may inject
  singletons, never the reverse and never another request-scoped. Reach
  one through the request boundary (today **HTTP only**:
  `nest_rs_http::Scoped<T>`), never a `#[inject]` field. GraphQL/MCP do
  not bridge the scope yet.
- **Modules compose by type or by configured value.** `#[module(imports
  = [...])]` takes a bare type (a static `Module`) or a call expression
  like `OpenApiModule::for_root(opts)` (a `DynamicModule` configured at
  its import site). A `DynamicModule` configures via `register` (sync)
  or `collect` (queues an async factory — a DB pool, a queue
  connection). Configuration is each module's responsibility, declared
  where it is imported, never seeded loosely in `main`. Registration is
  **idempotent** (a diamond import builds once); dynamic imports are
  not deduplicated.

### Encapsulation: compile-time + boot-time access contract

- **Visibility** is Rust's job: the container is flat (a provider is
  injectable by anyone who can name its type), so a feature hides its
  impl as module-private and exposes a `pub` **trait** bound with
  `provide_dyn`. Consumers inject `Arc<dyn Trait>`, never the impl.
- **The import contract** is enforced at boot by the access graph
  (`crates/nest-rs-core/src/access.rs`): `#[module]` records its imports
  and each provider's injected `TypeId`s into an `inventory` registry;
  `App` walks the graph from the root and **fails the boot**
  (`AccessGraphError`) if a provider injects something its module
  neither owns, imports transitively, nor receives as global
  infrastructure (seeds + factory outputs). It governs `#[inject]`
  fields **and** attribute-bound layers (`#[use_guards]` /
  `#[use_filters]` / `#[use_interceptors]`), which are
  container-resolved at mount. The one deliberate hole: runtime
  `Container::get`/`get_dyn` is an unchecked escape hatch — the
  contract binds the *declarative* surface, not imperative resolution.

### Discovery: `Discoverable` + per-method inventory

Anything a module wires up implements `Discoverable` and is listed in a
flat `#[module(providers = [...])]`. `#[injectable]`, `#[mcp]`, the
gateway struct, and similar single-concern decorators emit the single
`impl Discoverable` directly.

**For concerns that aggregate per-method units, the orchestrator
pattern applies uniformly:** `#[routes]` on a controller impl scans
`#[get]`/`#[post]`/…, `#[resolver]` on a resolver impl scans
`#[query]`/`#[mutation]`/`#[field]`, `#[scheduled]` on a provider impl
scans `#[every]`/`#[cron]`/`#[after]`, `#[processor]` on a provider
impl scans `#[process(queue, ...)]`. The host struct is an ordinary
`#[injectable]` (or `#[controller]`/`#[resolver]`) that owns
`Discoverable`; each method submits its own unit to a link-time
`inventory` registry so the orchestrator never collides with the
struct's single `Discoverable`. `#[hooks]` uses the same seam.

A new method-level decorator pair (orchestrator on impl + per-method
attribute) is the right model for any concern where one provider should
own several units sharing the same `#[inject]` dependencies. Anything
else — a transport-bound runtime, a single-concern marker — stays
struct-level.

**Discovery is module-gated.** Every transport (HTTP, GraphQL, WS,
Queue, Schedule, MCP, Events) integrates only items whose provider is
*reachable* from the running app's root module — a `ReachableProviders`
set computed from the access graph and seeded into the container;
each transport filters its `inventory` / metadata against it. Linking a
crate without importing its module = code present in the binary,
**inert** in this app — not pollution. A linked-but-unreachable
resolver fires a `tracing::warn` at boot (so leftover code does not
disappear silently) and is skipped from the schema. This is what makes
per-app subsets work: `apps/platform-worker` links `features` for the
data layer but importing only `UsersQueueModule` keeps the HTTP
controller, GraphQL resolver, and WS gateway out of the binary's
runtime surface.

GraphQL composition is **discovered, not listed**: each `#[resolver]`
submits its query/mutation objects to an `inventory` registry merged
into the schema roots at boot — no central `queries = [...]`. The
resolver *struct* is still listed in `providers` for the access
contract only. Batch field-resolver fetches with `#[dataloader]`
(request-scoped loaders) to avoid N+1s.

### Lifecycle hooks

`#[hooks]` on a provider's impl block submits each phase-tagged method
(`#[on_module_init]`, `#[on_application_bootstrap]`,
`#[on_module_destroy]`, …) to the same `inventory` registry; `App::run`
drains it per phase, resolving the provider from the container. Hooks
are **per-provider**, run in `(provider, method)` name order; init
failure aborts boot, shutdown is best-effort.

## Request layers: guards, filters, interceptors

A `Guard` borrows the request **mutably** — it gates access (return
`Err(Response)`) and may attach request context a handler reads back
with `nest_rs_http::Ctx<T>`. Bind guards/filters/interceptors three ways
— **global** (imperative), **controller** (on the struct), or
**handler** (beside the verb) — each container-resolved, no import,
first listed outermost. Per-route order, inner→outer: **shaper →
interceptors → guards → filters → meta**. Declarative per-handler
metadata a guard reads ships as `#[meta(EXPR)]` +
`nest_rs_http::Reflector`. The one asymmetry: **global** interceptors
wrap *outside* the global guards, because the data context must install
the executor/transaction around the guards too.

URI versioning via `#[controller(version = "1")]` mounts the controller
under `/v1` (`version_path` is the single source of truth).

## Authentication and authorization

`nest-rs-authn` answers *who the caller is*; `nest-rs-authz` answers
*what they may do*. They compose at the request boundary: bind
`#[use_guards(AuthGuard, AppAbilityGuard)]` — `AuthGuard` attaches the
principal, `AbilityGuard` builds the caller's `Ability`. Both the
verification alias and the policy live in `crates/features`
(`authn/`, `authz/` + `authz/http/`); apps only mount them.

A **`Strategy`** turns a request into a principal (a plain
`#[injectable]`, no macro). **`AuthGuard<S>`** is generic over it.
`Strategy::authenticate` returns an **`Outcome`**: `Authenticated` or
`Challenge` (a 401, or an OAuth redirect) — so one trait serves bearer
and OAuth. The standard resource-server case needs no app strategy:
`JwtStrategy<C>` ships it, so `features::authn::core` writes the alias
once (`type AuthGuard = AuthGuard<JwtStrategy<Claims>>`) and every
resource-server app mounts it. **`JwtService`** is global
infrastructure (factory phase); it takes a symmetric secret or an EdDSA
key pair — a resource server holds **only the public key** (cannot
mint tokens), which is why **token issuance is its own app**
(`apps/platform-auth` signs with the private key; `apps/platform-api`
is a pure resource server that only verifies). The two share
`crates/features` (the `identity` contract lives there) and the DB,
never RPC each other.

### Authz follows the port + adapters pattern

| Folder | Provides |
|--------|----------|
| `authz/` (root) | `AppAbility` (the policy), `AuthzModule` |
| `authz/http/` | `AppAbilityGuard` (`AbilityGuard<AppAbility>`), `AuthzHttpModule` |
| `authz/graphql/` | `AppGraphqlGuard` (`GraphqlAbilityBridge<AuthGuard, AppAbilityGuard>`) as `dyn OperationGuard`, `GraphqlAuthGuard` (`ResolverGuard` — access-graph marker), `LoaderScope` as `dyn BatchContext`, `AuthzGraphqlModule` + `forward_principal!(Claims)` |
| `authz/ws/` | `WsDataContext` as `dyn SocketContext`, `WsAuthGuard` (`MessageGuard` — access-graph marker), `AuthzWsModule` |

No app-side `authz/` folder — the bridges live with the rest of the
authz feature.

### One symmetric pattern across the three transports

Each transport surfaces its auth dep via the same mechanism:
`#[use_guards(<TransportAuthGuard>)]` on the handler kind that
transport exposes. Each feature's `<Feature><Transport>Module` then
imports its matching `Authz<Transport>Module` — **and only that** —
because the transport module transitively brings every layer it needs
(`AuthzHttpModule → AuthzModule → AuthnModule` for the underlying guards).

| Transport | Handler decorator | Auth guard binding | Module import |
|-----------|------------------|--------------------|---|
| HTTP | `#[controller]` | `#[use_guards(AuthGuard, AppAbilityGuard)]` on the impl block | `[<Feature>Module, AuthzHttpModule]` |
| GraphQL | `#[resolver]` | `#[use_guards(GraphqlAuthGuard)]` on the impl block | `[<Feature>Module, AuthzGraphqlModule]` |
| WS gateway | `#[gateway]` + `#[messages]` | `#[use_guards(AuthGuard, AppAbilityGuard)]` on the gateway struct (connection) + `#[use_guards(WsAuthGuard)]` on each `#[subscribe_message]` (per-message marker) | `[<Feature>Module, AuthzWsModule]` |

Every transport reduces to the **same two-line shape**: import the
feature's core, import the matching `Authz<Transport>Module`. The
transport's runtime infrastructure (`WsModule` for WS, the GraphQL
schema runtime, the HTTP transport itself) is reached transitively
through the authz module — no extra import per feature.

**Why an access-graph marker for GraphQL and WS, but real guards for
HTTP?** HTTP guards run on `&mut Request` *before* the handler —
they're the actual auth chain. GraphQL and WS run their authn/ability
at the operation guard (GraphQL `dyn OperationGuard`) or connection
level (WS HTTP upgrade guards), then seed `Ability` into the
per-operation context. A `ResolverGuard` / `MessageGuard` bound at the
handler is the **declarative seam** that turns this seeded-context
dependency into an `#[inject]` the access graph can validate: omit the
authz module from the feature's transport module and the boot fails
naming the missing guard.

**Public handlers** (no auth required) omit `#[use_guards(...)]` for
that transport, and accept that their module no longer imports the
matching `Authz<Transport>Module` transitively — the app must list it
explicitly if any other handler in the binary needs it.

## The data layer makes security and transactions transparent

The hardest promise — no hand-written row filter, no hand-written
transaction — is kept by a **request-scoped data context** held in two
`task_local!`s (a singleton service has no other way to read
per-request state):

- the **executor** (`nest-rs-seaorm`'s `Executor` enum: pool or transaction);
- the **ability** (`nest-rs-authz`'s ambient `Arc<Ability>`).

**Hard invariant: every data access goes through a service, and a
service reaches the DB only through `Repo`.** The service
(`CrudService`) is the entity's API and the single audited choke point
— controllers, resolvers, gateways, and dataloaders **delegate to it,
never touch `Repo` or the ORM directly** (resolver/gateway code — not
the service methods that implement batch loads). `CrudService`'s
`list`/`page`/`access`/`create`/`update`/`delete` each go through
`Repo` and emit a `nest_rs::orm` span (denials at `warn`). `Repo` runs
every query against the ambient executor (joining the request's
transaction with nothing threaded) and filters reads **and** by-id
writes by `condition_for` from the ambient ability — so a feature
cannot forget to scope what the caller may touch (no ability ⇒
`TRUE`, unscoped). By-id route-model binding goes through the gateway
too (`Bind`/`bind` delegate to `CrudService::access`).

The two task-locals install at different depths: the **executor**
outermost via the auto-registered `DbContext` interceptor (just import
`DatabaseModule`) — a safe method runs on the pool, a mutating one in
a transaction committed on 2xx/3xx and rolled back otherwise; the
**ability** inside the per-route guards, via the `#[routes]` **shaper**
(the only seam that runs after `AbilityGuard` and still wraps the
handler) — keeping `nest-rs-http` unaware of authz/ORM.

**HTTP response masking** (`nest-rs-authz` `http` feature / `Authorize`).
After a successful handler, the shaper parses the JSON body, runs
`Ability::mask` / `mask_many` on `Entity::Model`, and re-serializes.
Handlers should return the **`#[expose]` output type** (e.g.
`Json<User>`), not `Model` — Uuid fields are often `String` on the
wire. The shaper therefore: (1) parses the wire `Value`, (2) builds
`Model` via `wire_to_model` (filling the columns the wire DTO omits
with placeholders emitted by `#[expose]` as an `impl WireModelDefaults
for Entity` — one entry per `#[expose(skip)]` scalar column, typed by
the column's Rust type), (3) masks, (4) **`retain_wire_keys`** so an
unrestricted field grant cannot leak `#[expose(skip)]` columns (e.g.
`password_hash`). A body that cannot be reconciled with `Model` fails
**closed** with `500`, not unmasked data. Column types the macro
cannot default (e.g. `Decimal`, a custom enum) need a hand-written
`impl WireModelDefaults` next to the entity for that column.

Two HTTP extractors hand the handler a ready argument: **`Bind<S, A>`**
(parse id → load + authorize through the service: 404 absent, 403
denied) and **`Scope<E, A>`** (the explicit `Condition` for a handler
building its own query). A route using `Bind` must also bind an
`AbilityGuard`.

The same transparency extends past HTTP through **symmetric,
authz/ORM-agnostic seams** the surface crates expose. `nest-rs-authz`
exposes the authz-only bridges behind Cargo features — `http` (the
`Authorize` shaper, `AbilityGuard`, `Scope`), `graphql` (the
`GraphqlAbilityBridge` operation guard, the `authorize` gate, the
`ability` accessor), `mcp` (the `McpAbilityBridge`); the bridges that
also need the data layer (`Bind`, the GraphQL `bind` helper,
`LoaderScope`, `WsDataContext`) live in `nest-rs-seaorm` behind
matching `http`/`graphql`/`ws` features — that split keeps the engine
free of a circular dependency on the data layer. So GraphQL's
`OperationGuard` is `GraphqlAbilityBridge` (re-runs the guard chain on
`/graphql` only), `BatchContext` is `LoaderScope` (re-installs the
snapshotted ability + a **pool** executor around each off-task
dataloader batch), and WebSocket's `SocketContext` is `WsDataContext`
(installs the connection's pool + ability per message — no
per-message transaction). The **worker transports** install a pool
executor too via the orm-agnostic `JobContext` seam (`WorkerDbContext`,
auto-bound by `DatabaseModule`) — so a `#[scheduled]`/`#[processor]`
method gets an ambient `Repo` with no connection injected (system work
⇒ no ability ⇒ unscoped, correct). A genuinely contextless path (a
shutdown hook) keeps an injected `Arc<DatabaseConnection>` — the
**only** documented bypass of `Repo` on a provider.

**`#[dataloader]` batch methods** live on the service, use `Repo` like
any other read, and return `Result<HashMap<…>, E>` (or infallible only
when the method cannot fail). Never map a DB error to an empty batch
— that reads as success and violates the Rust-first rule.

Exemplar apps: `apps/platform-api` (REST + GraphQL + WS, DB + authz);
`apps/chat` (pure real-time).

## Surface crates — decisions, not mechanics

Each realizes the "new concern = new crate + decorator, no
`nest-rs-macros` change" claim. Read the crate for how; here is only
what was decided.

- **`nest-rs-schedule`** — `#[scheduled]` orchestrator on an
  `#[injectable]` provider's `impl` block; each method tagged with
  exactly one of `#[every]` (interval), `#[cron]` (cron expression,
  optional `tz`), or `#[after]` (one-shot). String literals validated
  at compile time; presets/timezones at boot — a bad value fails the
  boot naming the offending job. The `Scheduler` is a `Transport`
  contributed by `ScheduleModule` via `TransportContribution`.
- **`nest-rs-queue` + `nest-rs-redis`** — backend-agnostic queue contract
  (`nest-rs-queue`: the `Job`/`Processor`/`ProcessMethod` traits, the
  `#[processor]` macro, the inventory seam) with Redis as the first-class
  integration (`nest-rs-redis`, on `apalis` — the `@nestjs/bullmq` analog).
  Apps depend on both crates: `nest-rs-queue` for the abstractions and the
  macro, `nest-rs-redis` for the `QueueConnection` producer + the
  `QueueWorker` transport + the activation modules. Crate name follows the
  **storage the developer sees** (Redis), not the framework (apalis); a
  hypothetical NATS or SQS backend ships as its own `nestrs-<storage>`
  crate against the same `nest-rs-queue` contract. `#[processor]`
  orchestrates `#[process(queue, concurrency, retries)]` methods into
  per-storage consumers. Queues are **identified by name**
  (stringly-typed, the known cost). Producer and consumer are decoupled.
  Connection seeded as a factory at the root via `QueueModule::for_root`;
  the consumer runtime activates by importing the separate
  `QueueWorkerModule` (producer-only apps don't import it). No apalis
  types leak to apps.
- **`nest-rs-http`** — the only activation seam is
  `HttpModule::for_root(...)` in `AppModule.imports`; `App` has no
  public `.transport(...)` method. Every option of `HttpConfig { host,
  port, tls, … }` is settable both via `NESTRS_HTTP__*` env vars and
  via the pinned struct — the framework-wide **dual-path config rule**
  (applies to every `nestrs-*` module).
- **`nest-rs-pipes`** — transport-agnostic, **one `Pipe` per file**,
  stateless (`transform(In) -> Result<Out, _>`, never a DI provider).
  The base set covers the common cases (`Parse<T>`, `ParseUuid`,
  `ValidationPipe<T>`, …). HTTP binds them with the
  `Valid<E>` / `Piped<P, E>` extractors. Reusable pipes are framework
  primitives — never define one in an app.
- **`nest-rs-openapi`** — import `OpenApiModule`; it self-mounts
  `GET /api-json` (OpenAPI 3.1) + a bundled offline Swagger UI at
  `GET /api`. The document is **composed** from the route table, not
  listed. Payload schemas come from **schemars** (`Json<T>`'s
  `T: JsonSchema`); `#[api(...)]` enriches an operation.
- **`nest-rs-ws`** — **not a `Transport`**: a WS upgrade is an HTTP
  GET, so `#[gateway(path = "/ws")]` self-mounts on the existing
  `HttpTransport` (inheriting its port/CORS/TLS). `#[messages]`
  orchestrates `#[subscribe_message]` +
  `#[on_connect]`/`#[on_disconnect]`; one JSON envelope `{event, data}`.
  Guards bind at two scopes (connection-level `Guard`, per-message
  `MessageGuard`). Per-gateway namespacing via `WsServer<N>`.

## Naming rules — strict

The file name encodes the role; the folder supplies the feature prefix
(`users/service.rs`). Snake_case, role-prefixed by folder. No
dotted variants.

| Role | File name |
|------|-----------|
| DI module (one `<…>Module` struct + `#[module]` per file) | `module.rs` |
| Folder index (`pub use`, submod declarations only) | `mod.rs` |
| Service (business + DB gateway) | `service.rs` |
| Controller (REST endpoints) | `controller.rs` |
| Resolver (GraphQL) | `resolver.rs` |
| Gateway (WebSocket) | `gateway.rs` |
| Processor (queue consumer) | `processor.rs` |
| Tool (MCP) | `tool.rs` |
| Entity (ORM model + `#[expose]`) | `entity.rs` |
| DTO / Input types | `dto.rs` |
| Domain-specific error (a wire contract, an opaque security variant) | `error.rs` — only when the framework's `ServiceError` / `AuthError` / `TokenError` cannot carry it |
| GraphQL bridge type alias | `<feature>/graphql/bridge.rs` |
| Guard / Strategy (authn machinery) | `guard.rs` / `strategy.rs` |
| Static constants | `constants.rs` |

**Structural rules:**

- **`mod.rs` / `lib.rs` carry no business logic** — only `//!` doc,
  `mod`, and `pub use`. The one exception: a proc-macro crate's
  `#[proc_macro*]` entry functions (Rust forces them to the crate
  root) must be **thin delegations** to submodules. `mod.rs` is the
  folder index; `module.rs` is the DI module — **never merge them**.
- **One role → one file inside a folder.** Do **not** split one
  service into `loader.rs` / `credential.rs` unless a second pattern
  appears twice — keep the whole `UsersService` in `service.rs` (extra
  `impl` blocks for `CrudService`, `#[dataloader]`, `#[hooks]` are
  required by macros, not extra files). A single-role crate stays flat
  (`nest-rs-seaorm/config.rs`).
- **Multiple files of the same role ⇒ pluralized sub-folder.** Several
  `Pipe` impls in `nest-rs-pipes` go in `pipes/`; several `Strategy`
  impls in `nest-rs-authn/passport/` go in `strategies/`. The trait
  file (singular) stays at the parent (`pipe.rs`, `strategy.rs`);
  `pipes/mod.rs` / `strategies/mod.rs` re-export the impls flat.
- **Errors of a feature** belong in `error.rs` (or one clearly named
  module) — not scattered enums inside `service.rs`.
- **No `interfaces/` directory** — a trait lives in the file of its
  concern (or `traits.rs` / `types.rs` for a standalone cluster).
- **Apps live under `apps/<name>/`.** Not `examples/`, not
  `services/` — every runnable thing lives under `apps/` uniformly.
  By default an app contains only `main.rs` + `module.rs`; a feature
  folder under `apps/<x>/` is the documented exception.
- A file exists only if it has real content (a one-line role file is
  real content; this forbids empty placeholders, not small files).

## When (not) to write a new decorator

Reach for a macro when the boilerplate is mechanical, recurrent, and
unambiguous. Write code when it is none of the three.

**Write a new decorator when all three hold:**

- the same pattern appears in ≥ 3 places (two is coincidence);
- the boilerplate is mechanical — no business decision lives in it;
- the rule is teachable in one sentence (`#[mcp]` on a struct: it
  becomes a registered MCP tool).

**Do not write a decorator for:**

- business logic of any kind — that is a service;
- one-off integrations — write the `impl` directly;
- context-dependent type inference Rust cannot give us — resist; the
  macro leaks its limits to every call site. Prefer a builder;
- anything that needs `unsafe` or runtime reflection.

A new decorator must ship with: a doc comment showing the expanded
form, a test in the home crate's `tests/` (or in `nest-rs-testing` for
cross-crate wiring) proving the wiring, and a use site in an app or in
`features` so the integration is exercised end-to-end. Measure
incremental compile cost per use site; > 0.5 s is a defect
(see *North Star*).

## Engineering posture

- No premature abstraction. Extract after a pattern appears twice.
- Strict typing. Enums over string states. Parse at the edge with
  established crates (`validator`, `uuid` v7). Reserve newtypes for
  *meaning*, not format. Avoid `Box<dyn Any>` / `serde_json::Value`
  passthrough unless genuinely unstructured.
- Errors at boundaries: `thiserror` in libraries, `anyhow` at the app
  entry. No `unwrap()` on production paths. Propagate — do not
  log-and-pretend-success on data paths (especially dataloaders).
- Doc comments only where the *why* is non-obvious; never paraphrase
  the name.
- **Security is primordial**: access denials and security events log
  at `warn`+ (visible in prod), not `debug`.
- Every new third-party crate must have a published release within the
  last ~12 months. If a candidate fails this bar, **flag it
  explicitly** in the proposal. Never add a stale dependency silently.

## Observability conventions

- **Span targets are dotted, lowercase, framework-prefixed.**
  `nest_rs::http`, `nest_rs::orm`, `nest_rs::authn`, `nest_rs::authz`,
  `nest_rs::ws`, `nest_rs::queue`, `nest_rs::schedule`. Application spans
  use the app name (`api::users`, `auth::oauth`). One target per
  concern per crate.
- **Default level per layer.** Controllers / resolvers / gateways:
  `info` on success. Services: `debug` for ordinary calls. `Repo`:
  `trace` for queries. Access denials and security events: `warn`+.
  Unexpected errors: `error`. A hot path must respect `RUST_LOG=info`
  — no per-request `debug!` at info level.
- **Structured fields, not formatted strings.** Attach `actor_id`,
  `tenant_id`, `request_id` (via `Ctx<T>` where the framework exposes
  one) so a query filters cleanly. Prefer `user_id = %id` to
  `format!("user {} did X")`.
- **Production output is OTLP, not stdout.** The
  `nest-rs-opentelemetry` crate ships an OTLP appender; an app opts in
  via `OpenTelemetryModule`. Dev pretty-print is acceptable only
  under a `dev` profile.

## Testing — "done" means verified live

Wiring bugs do not surface in unit tests. Every app ships one
`apps/<app>/tests/e2e.rs` booting its real `AppModule` in-process
against the live devcontainer Postgres/Redis — add or extend it. For
HTTP/GraphQL changes that is still not enough: run the binary, `curl`
the affected endpoints, then **kill the server before returning
control**.

**Three categories** — Rust's standard model plus the app e2e:

- **Unit tests** — `#[cfg(test)] mod tests` inside the `src/` file
  under test. The default home for pure-logic assertions: parsers,
  formatters, mappings, any pure function or impl body. Private-item
  access is the point.
- **Integration tests** — `tests/*.rs` at the crate root, testing the
  crate's **public** API as an external consumer would. Cargo compiles
  each top-level `tests/*.rs` as its own binary — that is normal
  Cargo behavior, not a smell. Group related tests in one file when
  they share setup; split files when they don't. Shared helpers go in
  `tests/common/mod.rs` (the `mod.rs` form prevents Cargo from
  compiling it as a separate binary — official Rust idiom). No DB
  unless the crate truly owns persistence.
- **End-to-end tests** — exactly one `apps/<app>/tests/e2e.rs` per
  app: boots the real `AppModule` against live Postgres/Redis,
  exercising routes, DI wiring, and transports.

**Cross-crate framework wiring** lives as integration tests in
`nest-rs-testing` (`tests/*.rs`) — boot-time access-graph rejection,
lifecycle hook ordering, transport contribution.

**Test commands are `just`-driven recipes — three of them, no more:**

- `just test` — unit + integration (no DB);
- `just test-e2e` — e2e (live Postgres/Redis required);
- `just test-cov` — coverage on the full suite.

Gating is a nextest binary filter (`-E 'binary(e2e)'`), **not**
`#[ignore]`. Do not reintroduce `test-unit`.

**No mocking the database in e2e tests** — real Postgres
(testcontainers in CI). Unit tests of pure logic need no DB.

**Testability rule**: if a type is hard to test, fix the API (explicit
`new(deps)`, no leaked secrets in HTTP bodies) — do not skip coverage.

A GraphQL app commits its SDL (`apps/<app>/schema.graphql`),
regenerated as a side effect of the **dev run** (`emit_sdl` driven
from the environment) — there is no standalone generator and no CI
drift-check.

## Hard "no" list

The rules above forbid many things; this list is the short bar for
items that are non-obvious or recurring temptations.

- No external DI library.
- No renaming of `apps/` or `crates/features/`.
- No feature flags for capabilities that do not yet exist.
- No backwards-compatibility shims (no public API to preserve yet).
- No mocking the database in e2e tests.
- No umbrella module that imports every edge of a feature. Apps list
  edges explicitly so the imports table-of-contents reflects what the
  binary actually serves.
- No transport-level discovery without module-gating — every
  transport's inventory drain must filter by access-graph
  reachability.
- No two decorators that do the same thing — deprecate one before
  adding another.
- Multiple deployable apps split by responsibility are a goal (not
  microservices sprawl) under two conditions: apps share code through
  **crates** (never copy-paste — all product logic, contracts, and
  policy live in `crates/features`; see *Monorepo layout*), and the
  coupling stays **loose** (a self-contained token + the shared DB,
  never chatty RPC).

## Reading order for a new agent

This file plus the **code** are the source of truth. Past agent
transcripts are not injected automatically.

1. **This file** — durable rules.
2. **`crates/features/src/users/`** — the reference feature; copy
   before inventing. Read the feature root files, then any `<edge>/`.
3. **`apps/platform-api/`** — the reference app (REST + GraphQL + WS +
   DB + authz); `module.rs` is the canonical composition example.
4. **The relevant `crates/nestrs-<concern>/`** for whatever you are
   about to touch.

User-level rules configured in the IDE (e.g. "explain in French,
code/comments in English") apply per session.

## Workflow

State the plan in one or two sentences before invoking tools. Batch
independent tool calls in parallel. Run `just test` after meaningful
changes; `just test-e2e` if the change touches transports, DI wiring,
or persistence. For HTTP/GraphQL changes verify live by curling the
affected endpoints, then **kill any background server before returning
control**. Report what changed and what was verified — no
paragraph-long summary.

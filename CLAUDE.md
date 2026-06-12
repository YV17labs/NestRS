# CLAUDE.md — nestrs

For an LLM picking up this repo. The codebase tells you what *is*;
this file tells you what was **decided** and must be **respected**.
Doesn't re-document the code — layout, signatures, versions, mechanics
live there.

Public repo. No machine-local paths, no private references.

## What this project is

nestrs is an opinionated Rust framework whose thesis is **the developer
writes business logic; the framework carries the rest**. Cross-cutting,
error-prone concerns — **authn, authz, row-level filtering,
transactions, edge validation, discovery, lifecycle** — must be
*transparent*. Forcing the developer to hand-manage any of them is a
framework defect.

Leverage = **procedural macros** (decorators as declarative in Rust as
in TS). `crates/nestrs-*` = framework; `crates/features/` = product
vertical slices (port at the feature root + one adapter sub-folder per
transport: `http/`, `graphql/`, `ws/`, `queue/`, `mcp/`);
`apps/<name>/` = `main.rs` + `module.rs` composing edges.

## Rule priority — Rust first, conventions second

Both, in order. When they conflict, **Rust wins** — adapt the
convention, don't bend Rust.

1. **Rust (non-negotiable).** Idiomatic, reviewable: orphan/coherence,
   explicit errors (`thiserror` in libs — no silent failure, no
   swallowed `DbErr`), no `unwrap()` on production paths, honest APIs
   (`Type::new(deps)` when tests need it), `Result` propagated to the
   transport boundary. Macro-emitted `impl` blocks don't excuse hiding
   errors or bypassing `Repo` outside the exceptions named here.
2. **Conventions (second).** Module/feature folders, decorator names,
   thin handlers, one `service.rs` per feature. Conventions = *where*;
   Rust = *how*.

## Where nestrs departs from convention

Deliberate departures. Don't "fix" them back.

| Common habit | nestrs decision | Why |
|---|---|---|
| Umbrella module re-exporting every edge | Feature ships `UsersModule` (port) + one `Users<Edge>Module` per transport; apps list edges served | Imports reflect what the binary serves |
| Split service across topic files | One `service.rs` per feature — don't fragment | No premature decomposition |
| Return `[]` when DB fails | **Forbidden** — batch/loader methods return `Result` | Silent failure violates Rust-first |
| Per-feature error enum for plumbing | Framework owns it: `nest_rs_seaorm::ServiceError`, `nest_rs_authn::AuthError`/`CredentialError`/`TokenError`. A feature never redefines them | Features write own errors only for genuinely domain-specific wire contracts or security-opaque variants |
| `exports` list for service re-export | `pub trait` + module-private impl, injected as `Arc<dyn Trait>` | Rust visibility is the primitive |
| Per-method transaction decorator | Ambient `task_local!` executor wraps mutating handlers | No per-method ceremony |
| Per-module sub-container | Single flat container | Orphan rules prevent accidental coupling |
| Manual per-endpoint redaction | `Ability::mask` runs automatically after every handler | Forgetting is structurally hard |
| Hand-written per-transport DTOs (NestJS) | Annotate the entity: `#[expose]` opts a column onto the wire | The entity *is* the wire contract — no DTO to forget to update |
| Expose-all, hide with opt-out | **Opt-in:** a column crosses HTTP/GraphQL/WS only with `#[expose]`; silence = hidden | Fail-secure on schema evolution — a column added by a later migration never leaks by omission. Same posture as the write side (`input(...)` is already opt-in), now symmetric for reads |
| Listing every controller/provider | Inventory-based discovery | Module list = decorated things |
| Class-based DI with reflection | Type-id DI with `#[inject]` fields | Rust has no reflection |
| Implicit runtime access check | Compile-time + boot-time access graph | Boot fails with a clear graph error |
| `nest generate` scaffolding | `nestrs g feature/resource/<transport>` scaffolds + auto-wires (`nest-rs-cli`); still copy `users/`/`orgs/` to harden with `#[crud]` + authz | Generators kill the mechanical boilerplate; the exemplar stays the source for CRUD/authz depth |

## North Star — what "good" looks like

DX targets, not perf promises (Rust perf is the default).

- **New CRUD feature ≤ 60 lines** in `crates/features/<feature>/`.
  When that breaks, open an issue — don't rewrite the boilerplate.
- **Adding a feature = copying `crates/features/src/users/`.** If the
  copy isn't enough, fix the exemplar — don't invent a second pattern.
- **Security wired by composition, not ceremony.** Importing
  `DatabaseModule` + `Authz<Edge>Module` activates row-level
  filtering, transaction scope, and response masking. Handlers opt
  *out* by not importing. Guards still bind explicitly per route —
  principal source is a policy decision.
- **A decorator that adds > 0.5 s per use site is a defect.** Measure.
- **Zero `unwrap`/`expect` on framework hot paths.** Tests and one-shot
  bootstraps may use them.
- **One way to do a thing.** Deprecate before adding a second
  decorator for the same concern.

## Monorepo layout

Three homes. Dividing rule: `crates/features/` when *any other app
could reuse it*; `apps/<x>/` only when *this app's exposure decides
something the feature can't generalize*.

- **`crates/nestrs-*` — framework.** Generic, product-agnostic. Never
  names a concrete `Claims`, entity, or policy — generic *over* them.
- **`crates/features/` — product features.** Hexagonal per slice: port
  at the feature root (`entity.rs`, `service.rs`, `dto.rs`,
  `error.rs`, `module.rs`); each adapter is a sub-folder per
  transport with its own `module.rs`. Port at the root — not in a
  `core/` sub-folder — is deliberate.
- **`apps/<name>` — pure composition.** `main.rs` + `module.rs` only,
  by default. A feature folder under `apps/<x>/` is the exception
  (glue handler over several features, deployment-specific route).

**Port + adapters** (`users/`):

| Path | Contents | Module struct |
|---|---|---|
| `users/` (root) | `entity.rs`, `service.rs`, `dto.rs`, `error.rs`, `module.rs` | `UsersModule` (port) |
| `users/http/` | `controller.rs`, `error.rs` | `UsersHttpModule` |
| `users/graphql/` | `resolver.rs` (field + root merged into `UsersResolver`) | `UsersGraphqlModule` |
| `users/ws/` | `gateway.rs` | `UsersWsModule` (imports `WsModule` too) |
| `users/queue/` | `processor.rs` | `UsersQueueModule` |
| `users/mcp/` | `tool.rs` | `UsersMcpModule` |

Each adapter imports `UsersModule` explicitly — composition, not
inheritance. Importing only the port mounts no endpoint.

**One `#[module]` per folder.** DI file is **always** `module.rs`;
**exactly one** `#[module]` struct per file. Multiple modules per
feature ⇒ multiple folders. **No `*_module.rs` ever.** Pluralized
adapter folders (`pipes/`, `strategies/`) when several variants live
there; trait file stays at parent (`pipe.rs`, `strategy.rs`).

## Macros and the container

**Reach for macros first.** When wiring a service, module, or endpoint,
use the decorators. When a pattern recurs without one, write a new
decorator (threshold below in *When (not) to write a decorator*).

A `proc-macro` crate can only export macros, so each decorator lives in
a companion `*-macros` crate re-exported by its home crate. Shared
token helpers in `nest-rs-codegen`. A `*-macros` crate **must not**
depend on its surface crate — emit absolute-path tokens
(`::nest_rs_core::*`, `::std::sync::Arc`); never rely on call-site scope.

**Controllers are thin.** A handler wires layers, each with one home:
**Guard** (gates access, attaches context), **Pipe** (stateless edge
conversion/validation), **Bind** (id → loaded + authorized entity),
**Service** (business + sole DB gateway), **Interceptor** (cross-cutting,
e.g. transaction wrapping). Inline conversion, perm checks, or txn
management in a handler is drift.

**The DI container is internal.** Surveyed the ecosystem; none met our
bar. **Do not propose an external DI crate.** Extend ours.

### Composition model

- **`App::builder().build().await` runs four phases** independent of
  call order: *seeds* (runtime values from `main`), *collect* (modules
  queue async factories), *factories* (awaited; seed wins over factory
  of same type), *register* (providers built, injecting seeds +
  factory outputs). `main` holds only
  `App::builder().module::<AppModule>()` (+ transports). Sync apps
  keep `App::new`.
- **Providers are singletons** unless `#[injectable(scope = request)]`
  — built per request, deps from the singleton root. **One level
  deep**: request-scoped may inject singletons; never the reverse or
  another request-scoped. Reach one through the request boundary
  (today **HTTP only**: `nest_rs_http::Scoped<T>`), never via
  `#[inject]`.
- **Modules compose by type or configured value.** `#[module(imports =
  [...])]` takes a bare type or a call like
  `OpenApiModule::for_root(opts)` (`DynamicModule`). Configure via
  `register` (sync) or `collect` (async factory). Registration is
  **idempotent** (diamond imports build once); dynamic imports aren't
  deduplicated.

### Access contract (compile-time + boot-time)

- **Visibility** = Rust's job. Flat container ⇒ hide impls
  module-private, expose `pub trait` bound with `provide_dyn`.
  Consumers inject `Arc<dyn Trait>`.
- **Import contract** enforced at boot by the access graph
  (`crates/nest-rs-core/src/access.rs`): `#[module]` records imports
  and each provider's injected `TypeId`s into `inventory`; `App`
  walks from the root and fails boot (`AccessGraphError`) if a
  provider injects something its module doesn't own, import
  transitively, or receive as global infra (seeds + factory outputs).
  Governs `#[inject]` **and** `#[use_guards]`/`#[use_filters]`/
  `#[use_interceptors]`. Runtime `Container::get`/`get_dyn` is an
  unchecked escape hatch — contract binds the declarative surface only.

### Discovery

Module-wired items implement `Discoverable`; modules list flat in
`#[module(providers = [...])]`. Single-concern decorators
(`#[injectable]`, `#[mcp]`, gateway struct) emit `impl Discoverable`
directly.

**Orchestrator pattern for per-method aggregation:** `#[routes]` scans
verbs, `#[resolver]` scans `#[query]`/`#[mutation]`/`#[field_resolver]`,
`#[scheduled]` scans `#[every]`/`#[cron]`/`#[after]`, `#[processor]`
scans `#[process(queue, ...)]`, `#[hooks]` scans phase attrs. Host
struct owns the single `Discoverable`; each method submits its unit to
link-time `inventory`. Use this pattern for any concern where one
provider owns several units sharing the same `#[inject]` deps.
Otherwise stay struct-level.

**Discovery is module-gated.** Every transport integrates only items
whose provider is *reachable* from the running app's root — a
`ReachableProviders` set from the access graph; each transport filters
its `inventory` against it. Linked but unreachable ⇒ inert (boot
`tracing::warn` so leftover code doesn't disappear silently). This is
what makes per-app subsets work.

GraphQL composition is **discovered, not listed**: each `#[resolver]`
submits its objects to `inventory`, merged into the schema at boot.
The resolver struct is still listed in `providers` for the access
contract only. Batch field fetches with `#[dataloader]`
(request-scoped) to avoid N+1.

### Lifecycle hooks

`#[hooks]` submits phase-tagged methods (`#[on_module_init]`,
`#[on_application_bootstrap]`, `#[on_module_destroy]`, …) to
`inventory`; `App::run` drains per phase. Per-provider, run in
`(provider, method)` name order; init failure aborts boot, shutdown
is best-effort.

## Request layers — one pool, exactly once

**The invariant.** Declaring a layer (guard / pipe / interceptor /
filter / exception-filter) at any scope — **global** (imperative
`use_*_global`), **controller** (on the struct), **handler** (beside
the verb) — contributes to ONE pool per family, deduplicated by
`TypeId` through `compose_chain` (`nest-rs-core/src/layer_chain.rs`,
the single dedup logic for all five families). The layer executes
**exactly once per request**; broadest scope wins; `#[force_*]` is the
re-run opt-in. Scope never multiplies executions — it chooses the
**execution site**, matched to the family's nature:

| Family | Site (global scope) | Site (controller/method) |
|---|---|---|
| Guard | `RouteShaper` (post-routing — reads `#[public]`); `Guarded` self-mount edge; in-band `/graphql` op-guard | same sites |
| Pipe | `RouteShaper` | `RouteShaper` |
| ExceptionFilter | route site (typed catch, closest to handler) | route site |
| Interceptor | **transport edge** (band 90) — sees 404s, denials, self-mounts; runs *before* auth (no principal/ability/executor) | around the handler, *inside* guards |
| Filter | **transport edge** (band 50) | around the handler, *inside* guards |

Teachable rule: *global = around the whole HTTP process; scoped =
around your handler; either way, once.* `Layer::priority` orders
entries *within* a site, never across sites.

Per-route inner→outer (from `#[routes]`): **handler → ability shaper →
exception-filter pool → scoped filters → scoped interceptors →
RouteShaper (guard pool → pipe pool) → `#[meta]`/`#[public]` (route
data)**. Transport bands (innermost→outermost): **routing → DbContext
(−10) → global filter pool (50) → global interceptor pool (90) → infra
`#[interceptor]` (100)**. Same relative nesting at both sites:
interceptors outside filters, exception-filters closest to the handler.
Two ways to be transport-wide, deliberately: `use_*_global` = the
**pool** (app-listed, TypeId-deduped against narrower scopes);
`#[interceptor]` = **infra** a module import brings (auto-mounted, off
pool, fixed band — `DbContext`, tracing, timing).

A `Guard` borrows the request **mutably** — gates access (returns
`Err(Denial)`), may attach context read back via `nest_rs_http::Ctx<T>`.
Denials are `Ok(4xx)` responses, never `Err` — filters don't see them;
global interceptors observe them. Per-handler metadata via
`#[meta(EXPR)]` + `nest_rs_http::Reflector`.

**Fail-secure boot.** Specs resolve at `configure`: an unresolvable
global spec (provider's module not imported) **fails boot** naming the
type (`HttpBootCheck`) — never a silent drop. An imperative
`HttpTransport::mount(...)` under active global guards fails boot too
(`fail_secure_strict`, default `true`; `false` downgrades to warn).
Self-mounts declare an `EdgePosture`: `Guarded` (default — WS upgrade)
gets the global chain at its edge; `Exempt` (graphql / mcp / openapi)
gates in-band or is deliberately public. `/graphql` stays fail-secure
under `Exempt` through the **fallback operation guard**: with no
registered `GraphqlOperationGuard`, the global guard pool runs in-band
per operation (a registered bridge *replaces* it — it runs the same
guards itself, so nothing double-runs). The graphql endpoint's `Public`
data marker is load-bearing: it lets `AuthGuard` admit anonymous
operations through to resolver gates.

**Mapped errors never commit.** A route-site `Filter`/`ExceptionFilter`
that maps a handler `Err` to a response tags it
`nest_rs_core::MappedError`; `DbContext` rolls back regardless of the
mapped status. (Global filters sit outside `DbContext` — the rollback
already happened.)

URI versioning: `#[controller(version = "1")]` mounts under `/v1`
(`version_path` is the single source of truth).

## Authn / authz

`nest-rs-authn` answers *who*; `nest-rs-authz` answers *what they may
do*. Compose at the boundary: `#[use_guards(AuthGuard,
AuthzGuard)]`. Verification alias and policy live in
`crates/features` (`authn/`, `authz/` + `authz/http/`); apps only mount.

**`Strategy`** turns a request into a principal (plain `#[injectable]`,
no macro). **`AuthGuard<S>`** is generic over it.
`Strategy::authenticate` returns `Result<Self::Principal, AuthError>`
— a pure request → principal mapping that **never issues a transport
response**; a redirect-style flow (OAuth `/authorize`) is a plain
handler, so one trait serves bearer and OAuth alike. Standard
resource-server: `JwtStrategy<C>` ships it; `features::authn::core`
writes `type AuthGuard = AuthGuard<JwtStrategy<Claims>>` once.
**`JwtService`** is global infra (factory phase); symmetric secret or
EdDSA key pair — a resource server holds **only the public key**
(can't mint tokens). So **token issuance is its own app**
(`apps/auth` signs; `apps/api` only verifies). They
share `crates/features` and the DB, never RPC each other.

### Authz follows port + adapters

| Folder | Provides |
|---|---|
| `authz/` (root) | `AppAbility`, `AuthzModule` |
| `authz/http/` | `AuthzGuard` (`AbilityGuard<AppAbility>` — **alias in `features`, not in `nest-rs-authz`**), `AuthzHttpModule` |
| `authz/graphql/` | `AppGraphqlGuard` (`GraphqlAbilityBridge<…>`) as `dyn OperationGuard`, `GraphqlAuthGuard` (`ResolverGuard` marker), `LoaderScope` as `dyn BatchContext`, `AuthzGraphqlModule` + `forward_principal!(Claims)` |
| `authz/ws/` | `WsDataContext` as `dyn SocketContext`, `WsAuthGuard` (`MessageGuard` marker), `AuthzWsModule` |

No app-side `authz/` folder — bridges live with the rest of authz.

### Symmetric pattern across transports

Each feature's `<Feature><Transport>Module` imports its matching
`Authz<Transport>Module` — **and only that** (transports transitively
bring every layer they need).

| Transport | Handler | Guard binding | Module import |
|---|---|---|---|
| HTTP | `#[controller]` | `#[use_guards(AuthGuard, AuthzGuard)]` on impl | `[<Feature>Module, AuthzHttpModule]` |
| GraphQL | `#[resolver]` | `#[use_guards(GraphqlAuthGuard)]` on impl | `[<Feature>Module, AuthzGraphqlModule]` |
| WS | `#[gateway]` + `#[messages]` | `#[use_guards(AuthGuard, AuthzGuard)]` on gateway struct + `#[use_guards(WsAuthGuard)]` on each `#[subscribe_message]` | `[<Feature>Module, AuthzWsModule]` |

**Why markers (not real guards) for GraphQL/WS?** HTTP guards run on
`&mut Request` before the handler — they *are* the auth chain.
GraphQL/WS run authn/ability at the operation guard / connection
upgrade, then seed `Ability` into per-operation context. The marker
turns that seeded-context dep into an `#[inject]` the access graph can
validate: omit the authz module ⇒ boot fails naming the missing guard.

**Public handlers** omit `#[use_guards(...)]` for that transport and
lose the transitive `Authz<Transport>Module` import — the app must
list it explicitly if other handlers need it.

## Data layer (transparent security + transactions)

Two request-scoped `task_local!`s (singletons have no other way to
read per-request state):

- **executor** (`nest-rs-seaorm` `Executor`: pool or transaction);
- **ability** (`nest-rs-authz` ambient `Arc<Ability>`).

**Hard invariant: every data access goes through a service; a service
reaches the DB only through `Repo`.** `CrudService` is the entity's
API and single audited choke point — controllers, resolvers,
gateways, dataloader resolver code **delegate, never touch `Repo` or
the ORM directly**. `CrudService::list`/`page`/`access`/`create`/
`update`/`delete` go through `Repo`, emit `nest_rs::orm` spans
(denials at `warn`). `Repo` runs every query against the ambient
executor and filters reads **and** by-id writes by `condition_for`
from the ambient ability (no ability ⇒ `TRUE`, unscoped). Route-model
binding goes through the service (`Bind`/`bind` delegate to
`CrudService::access`).

Install depths: **executor** via the auto-registered `DbContext`
interceptor (just import `DatabaseModule`) — innermost transport band
(−10), wrapping routing, so it covers controllers and self-mounts alike.
Safe methods run on the pool; mutating methods in a transaction —
commit on 2xx/3xx, rollback otherwise **and** on any
`MappedError`-tagged response. Guards run *inside* it (post-routing):
a denied mutation opens an empty txn that rolls back — fail-secure
holds; the wasted `BEGIN`/`ROLLBACK` is the accepted cost of guards
reading `#[public]` after routing (lazy executor = the planned fix).
**Ability** installs inside per-route guards via the `#[routes]` shaper
(only seam that runs after `AbilityGuard` and still wraps the handler)
— keeps `nest-rs-http` unaware of authz/ORM.

**HTTP response masking** (`nest-rs-authz` `http` / `Authorize`).
After success: parse JSON body → build `Model` via `wire_to_model`
(filling the **unexposed** columns the wire DTO omits from `impl
WireModelDefaults for Entity` emitted by the macro) →
`Ability::mask`/`mask_many` → **`retain_wire_keys`** (unrestricted
field grants can't leak unexposed columns). Handlers return the
`#[expose]` output (e.g. `Json<User>`), not `Model`. Irreconcilable
body ⇒ fail **closed** with `500`. Reconstruction needs a default for
every unexposed column: the macro provides one for the safe scalar
types (`String`/`Option`/`bool`/numbers); a hidden column of a type it
can't default (`Uuid`, timestamps, `Decimal`, custom enums) needs a
hand-written `impl WireModelDefaults`, so columns an ability rule
predicates on are best left exposed.

Two HTTP extractors: **`Bind<S, A>`** (parse id → load + authorize via
service: 404 absent, 403 denied) and **`Scope<E, A>`** (explicit
`Condition` for hand-built queries). Routes using `Bind` must also
bind an `AbilityGuard`.

Same transparency past HTTP via authz/ORM-agnostic seams.
`nest-rs-authz` exposes authz bridges behind features — `http`
(`Authorize`, `AbilityGuard`, `Scope`), `graphql`
(`GraphqlAbilityBridge`, `authorize`, `ability`), `mcp`
(`McpAbilityBridge`); data-layer bridges live in `nest-rs-seaorm`
behind matching `http`/`graphql`/`ws` features (`Bind`, GraphQL
`bind`, `LoaderScope`, `WsDataContext`) — split avoids a circular dep.
GraphQL `OperationGuard` = `GraphqlAbilityBridge` (re-runs guard
chain on `/graphql`), `BatchContext` = `LoaderScope` (snapshots ability
+ pool executor around each off-task dataloader batch), WS
`SocketContext` = `WsDataContext` (pool + ability per message — no
per-message transaction). **Worker transports** install pool via
orm-agnostic `JobContext` (`WorkerDbContext`, auto-bound by
`DatabaseModule`) — system work ⇒ no ability ⇒ unscoped, correct.
A truly contextless path (shutdown hook) keeps an injected
`Arc<DatabaseConnection>` — the only `Repo`-*less* bypass (no executor
at all). Two ability-less paths stay **inside** `Repo` via
`Repo::unscoped()` / `unscoped_by_id()`: pre-authentication credential
lookup (no principal yet ⇒ no ability) and `CrudService::access` (must
distinguish `Denied` from `Missing`, so it filters by ability
explicitly after the unscoped load). Every other read uses
`scoped`/`all`/`find_by_id`, which apply the ambient ability `WHERE`.

**`#[dataloader]` batch methods** live on the service, use `Repo`,
return `Result<HashMap<…>, E>` (infallible only when truly cannot
fail). Never map a DB error to an empty batch.

**Relations resolve themselves.** A SeaORM `#[sea_orm(belongs_to, …)]`
or `#[sea_orm(has_many)]` field **marked `#[expose]`** on an `#[expose]`d
entity becomes a GraphQL field auto-resolved by a dataloader. `#[expose(name = "…",
service = <Path>)]` emits the PK loader (`<Service>ById`) on the
service for every entity, the FK loader (`<Service>By<FkCol>`) per
`belongs_to` on the FK-owning side, the `PkLoadable` / `RelatedTo<Parent>`
trait impls that let the inverse side reach the loader **without
naming the other service**, and a `#[ComplexObject]` field resolver on
the wire DTO. Every batch goes through `Repo::scoped(Action::Read)`,
so an `Ability` filter applies row-level as on any other read.
omitting `#[expose]` on a single relation opts that field out — the user
writes a `#[field_resolver]` if they need a custom shape (cursor connection,
extra filter). Cross-entity rule still holds: a service touching another
entity injects that entity's service; **the FK loader is part of its
owner's service, never the consumer's**. **One caveat:** async-graphql
allows at most one `#[ComplexObject]` per wire type, so a custom
`#[field_resolver]` on the resolver cannot live next to an auto-resolved
relation on the same entity — pick one source per `ComplexObject`.

Exemplar apps: **Publish** workspace — `apps/api` (REST + GraphQL +
DB + authz), `apps/live` (WebSockets), `apps/auth` (issuer),
`apps/assistant` (MCP), `apps/worker` (queue). Simple
hello/blog layouts are CLI-scaffolded only — see docs, not hosted in this repo.
Tutorial feature exemplar: `crates/features/src/posts/`.

## Surface crates — decisions, not mechanics

- **`nest-rs-schedule`** — `#[scheduled]` orchestrator; methods tagged
  with exactly one of `#[every]` / `#[cron]` (optional `tz`) /
  `#[after]`. Literals validated at compile time; presets/timezones
  at boot. `Scheduler` is a `Transport` via `TransportContribution`.
- **`nest-rs-queue` + `nest-rs-redis`** — backend-agnostic queue
  contract (`Job`/`Processor`/`ProcessMethod` traits + `#[processor]`
  + inventory seam) with Redis first-class (on `apalis`). Apps depend
  on both. Crate names follow the **storage** (Redis), not the
  framework (apalis). Queues identified by name (stringly-typed,
  known cost). Producer/consumer decoupled. Connection seeded via
  `QueueModule::for_root`; consumer activates via `QueueWorkerModule`
  (producer-only apps skip it). No apalis types leak.
- **`nest-rs-http`** — only activation seam is
  `HttpModule::for_root(...)` in imports; no public
  `.transport(...)`. Every `HttpConfig` field settable via
  `NESTRS_HTTP__*` env **and** the pinned struct — framework-wide
  **dual-path config rule** (applies to every `nestrs-*` module).
- **`nest-rs-pipes`** — transport-agnostic, **one Pipe per file**,
  stateless (`transform(In) -> Result<Out, _>`, never a DI provider).
  Base set covers common cases (`Parse<T>`, `ParseUuid`,
  `ValidationPipe<T>`, …); HTTP binds via `Valid<E>` / `Piped<P, E>`.
  Reusable pipes are framework primitives — never define one in an app.
- **`nest-rs-openapi`** — import `OpenApiModule`; self-mounts
  `GET /api-json` + offline Swagger UI at `GET /api`. Document
  **composed** from the route table. Schemas via **schemars**;
  `#[api(...)]` enriches an op.
- **`nest-rs-ws`** — **not a `Transport`**: WS upgrade is an HTTP
  GET, so `#[gateway(path = "/ws")]` self-mounts on `HttpTransport`
  (inherits port/CORS/TLS). `#[messages]` orchestrates
  `#[subscribe_message]` + `#[on_connect]`/`#[on_disconnect]`; one
  envelope `{event, data}`. Guards at two scopes (connection `Guard`,
  per-message `MessageGuard`). Per-gateway namespace via `WsServer<N>`.

## Naming — strict

File name = role; folder = feature prefix (`users/service.rs`).
Snake_case, no dotted variants.

| Role | File |
|---|---|
| DI module (one `<…>Module` + `#[module]` per file) | `module.rs` |
| Folder index (`pub use`, `mod` only) | `mod.rs` |
| Service | `service.rs` |
| Controller (REST) | `controller.rs` |
| Resolver (GraphQL) | `resolver.rs` |
| Gateway (WS) | `gateway.rs` |
| Processor (queue) | `processor.rs` |
| Tool (MCP) | `tool.rs` |
| Entity (ORM + `#[expose]`) | `entity.rs` |
| DTO / Input types | `dto.rs` |
| Domain-specific error (only when framework errors can't carry it) | `error.rs` |
| GraphQL bridge type alias | `<feature>/graphql/bridge.rs` |
| Guard / Strategy | `guard.rs` / `strategy.rs` |
| Static constants | `constants.rs` |

- **`mod.rs` / `lib.rs` carry no business logic** — only `//!` doc,
  `mod`, `pub use`. Exception: proc-macro `#[proc_macro*]` entries
  (Rust forces them at the crate root) must be thin delegations.
  `mod.rs` is the folder index; `module.rs` is the DI module — never
  merge.
- **One role → one file per folder.** Don't split a service into
  `loader.rs`/`credential.rs` unless a second pattern appears twice.
  Extra `impl` blocks for `CrudService`, `#[dataloader]`, `#[hooks]`
  are macro requirements, not extra files. Single-role crates stay flat.
- **A service's type ends in `Service`; one service per `service.rs`.**
  Absolute: a business-logic provider whose name doesn't end in
  `Service` is mis-modeled — rename it, or it isn't a service. One
  responsibility per service ⇒ one service per file; genuinely distinct
  responsibilities become distinct services, never two structs crammed
  into one `service.rs`. Being injectable doesn't make a provider a
  service: a client, connection, config, guard, strategy, or pipe is a
  *plain provider* and keeps a role-descriptive name.
- **Injected service field/variable = `svc`, else `<name>_svc`.** A
  struct (or test) with exactly one service dependency names it `svc`;
  with several — or any ambiguity — use the explicit suffix
  `<name>_svc` (`users_svc: Arc<UsersService>`, `jwt_svc:
  Arc<JwtService>` — `JwtService` ends in `Service`, so it counts).
  Non-service deps keep descriptive names (`db: Arc<DatabaseConnection>`,
  `queue: Arc<QueueConnection>`, `oauth: Arc<OAuth2Client>`,
  `config: Arc<IssuerConfig>`).
- **Same-role plural ⇒ pluralized sub-folder** (`pipes/`,
  `strategies/`). Trait file (singular) stays at parent; sub-folder's
  `mod.rs` re-exports flat.
- **Errors in `error.rs`** — not scattered enums inside `service.rs`.
- **No `interfaces/` directory** — trait lives with its concern (or
  `traits.rs` / `types.rs` for a standalone cluster).
- **Apps under `apps/<name>/`** — not `examples/`, not `services/`.
  Default: `main.rs` + `module.rs`. Feature folder is the exception.
- A file exists only if it has real content.

## When (not) to write a decorator

**Write a decorator when all three hold:** pattern appears in ≥ 3
places; boilerplate is mechanical; rule teachable in one sentence.

**Do not write a decorator for:** business logic; one-off integrations;
context-dependent inference Rust can't give (prefer a builder);
anything needing `unsafe` or runtime reflection.

A new decorator ships with: doc comment showing expansion; a test in
the home crate's `tests/` (or `nest-rs-testing` for cross-crate
wiring); a use site in an app or `features`. Compile cost per use
site > 0.5 s = defect.

## Engineering posture

- No premature abstraction. Extract after a pattern appears twice.
- Strict typing. Enums over string states. Parse at the edge
  (`validator`, `uuid` v7). Newtypes for *meaning*, not format. Avoid
  `Box<dyn Any>` / `serde_json::Value` passthrough.
- Errors at boundaries: `thiserror` in libs, `anyhow` at app entry. No
  `unwrap()` on production paths. Propagate — don't
  log-and-pretend-success (especially in dataloaders).
- Doc comments only when the *why* is non-obvious; never paraphrase
  the name.
- **Security is primordial**: access denials and security events log
  at `warn`+, not `debug`.
- Every new third-party crate must have a release within ~12 months.
  Failing candidates must be **flagged explicitly**. No silent stale
  deps.

## Observability

- **Span targets dotted, lowercase, framework-prefixed.**
  `nest_rs::http`, `nest_rs::routes`, `nest_rs::orm`, `nest_rs::authn`,
  `nest_rs::authz`, `nest_rs::ws`, `nest_rs::queue`,
  `nest_rs::schedule`. App spans use the app name (`api::users`). One
  target per concern per crate.
- **Level per layer.** Controllers/resolvers/gateways: `info` on
  success. Services: `debug`. `Repo`: `trace`. Denials/security:
  `warn`+. Unexpected errors: `error`. Hot paths respect
  `RUST_LOG=info`.
- **Message + fields, never interpolation.** Production output is JSON,
  so every event is split: a **constant, general message** (the event
  name — `"mounted route"`, not `"GET /v1/users mounted"`) plus the
  dynamic data as **structured fields** that become JSON keys. Never
  bake values into the message string or hand-format columns (`{:<6}`)
  — alignment is the dev pretty-printer's job, and a baked string is
  unqueryable once it's JSON. Attach `actor_id`, `tenant_id`,
  `request_id`; prefer `method = verb, path = %p` to
  `format!("{verb} {p}")`. A list belongs in one field
  (`routes = list.join(", ")`), not the message.
- **Metadata is mandatory — a bare log is a defect.** Every event
  carries at least one structured field. A `tracing::info!("…")` with
  no fields is unqueryable once it's JSON and must not pass review —
  fields are how the event is filtered, joined, and correlated, not a
  nicety. Attach whatever tracing context the call site already holds
  (`actor_id`, `tenant_id`, `request_id`, `entity`/`id`, `signal`,
  `count`). The intolerable case is a security or denial event
  (`warn`+) emitted bare: those are exactly the events queried under
  incident, so a denial with no `actor_id`/resource field is a
  security gap, not a style nit.
- **One event, said once.** Don't restate in the message what a field
  or the enclosing span already carries, and don't emit the same event
  at two layers (a service `warn` plus a transport `warn` for one
  failure is duplicate noise — log it at its source layer per *Level
  per layer*). The message stays a short, constant event name:
  meaningful but never a sentence — the fields carry the specifics.
- **Production output is OTLP, not stdout.** `nest-rs-opentelemetry`
  ships an appender; app opts in via `OpenTelemetryModule`. Dev
  pretty-print only under a `dev` profile.

## Testing — "done" means verified live

Wiring bugs don't surface in unit tests. Every app ships one
`apps/<app>/tests/e2e.rs` booting its real `AppModule` against live
Postgres/Redis. For HTTP/GraphQL changes that's still not enough:
run the binary, `curl` the affected endpoints, then **kill the
server before returning control**.

**Three categories:**

- **Unit** — `#[cfg(test)] mod tests` inside the file under test.
  Home for pure-logic assertions; private-item access is the point.
- **Integration** — `tests/*.rs` at the crate root, testing the
  **public** API. Cargo compiles each as its own binary (normal).
  Shared helpers in `tests/common/mod.rs` (the `mod.rs` form prevents
  standalone compilation). No DB unless the crate owns persistence.
- **E2E** — exactly one `apps/<app>/tests/e2e.rs` per app: real
  `AppModule` against live Postgres/Redis.

Cross-crate framework wiring lives as integration tests in
`nest-rs-testing` (access-graph rejection, hook ordering, transport
contribution).

**The `test` recipe group** — defined in `test.just` (`mod test`), run
through `nestrs run` (the single front door, which forwards to `just`).
Bare `nestrs run test` **lists** the kinds (like `db`); pick one:

- `nestrs run test unit` — unit + integration + doctests (no DB);
- `nestrs run test e2e` — e2e (live Postgres/Redis);
- `nestrs run test cov` — coverage on the full suite;
- `nestrs run test doc` — doctests only.

`nextest` does not run doctests, so `unit` adds `cargo test --doc`
explicitly — otherwise doc examples never run. Gating is a nextest
binary filter (`-E 'binary(e2e)'`), **not** `#[ignore]`. The keyword is
`unit` (not a flat `test`/`test-unit` recipe).

**No mocking the database in e2e tests** — real Postgres
(testcontainers in CI). **Testability rule**: if a type is hard to
test, fix the API.

GraphQL apps commit their SDL (`apps/<app>/schema.graphql`),
regenerated as a side effect of the dev run (`emit_sdl` from env) —
no standalone generator, no CI drift-check.

## Hard "no" list

- No external DI library.
- No renaming of `apps/` or `crates/features/`.
- No feature flags for capabilities that don't yet exist.
- No backwards-compatibility shims (no public API to preserve yet).
- No mocking the database in e2e tests.
- No umbrella module importing every edge of a feature.
- No transport-level discovery without module-gating.
- No two decorators that do the same thing — deprecate first.
- Multiple deployable apps split by responsibility are a goal (not
  microservices sprawl) under two conditions: share code through
  **crates** (never copy-paste — product logic in `crates/features`)
  and keep coupling **loose** (self-contained token + shared DB, never
  chatty RPC).

## Reading order for a new agent

This file plus the **code** are the source of truth.

1. **This file** — durable rules.
2. **`crates/features/src/users/`** — reference feature; copy before
   inventing.
3. **`apps/api/`** — reference app (REST + GraphQL + DB
   + authz); `module.rs` is canonical composition.
4. **`crates/nestrs-<concern>/`** for whatever you touch.

User-level IDE rules (e.g. "explain in French, code/comments in
English") apply per session.

## Workflow

State the plan in one or two sentences before tools. Batch independent
calls in parallel. Run `nestrs run test unit` after meaningful changes;
`nestrs run test e2e` if the change touches transports, DI wiring, or
persistence. For HTTP/GraphQL changes verify live by curling, then
**kill any background server before returning control**. Report what
changed and what was verified — no paragraph-long summary.

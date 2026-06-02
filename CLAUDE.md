# CLAUDE.md — nestrs

For an LLM picking up this repository. The codebase tells you what *is*; this
file tells you what was **decided** and what must be **respected**. It does not
re-document the code — crate layout, macro signatures, dependency versions, and
mechanics live in the code, which is authoritative. Read it there.

This file is committed to a public repository. No machine-local paths, no
private references.

## What this project is

nestrs is an opinionated Rust framework whose central bet is **procedural
macros**. Crates under `crates/nestrs-*` provide the framework building blocks
(IoC container, module trait, the decorator macros); `crates/domain` holds the
product's shared core; binaries under `apps/<name>/` are thin entry points that
consume both. *Monorepo layout* below is the law on which of the three a given
file belongs to.

NestJS inspired the surface; it is the **porting mental model**, not the spec.
Describe features by what they do in nestrs; use Nest vocabulary only when it
helps someone map decorators and folders.

## Rule priority — Rust first, Nest second

Every change must satisfy **both**, in this order. When they conflict, **Rust
wins** — adapt the Nest mapping, do not bend idiomatic Rust or the type system.

1. **Rust (non‑negotiable).** Idiomatic, reviewable Rust: orphan/coherence rules,
   explicit errors (`thiserror` in libraries — no silent failure, no swallowed
   `DbErr` in loaders), no `unwrap()` on production paths, honest APIs
   (`Type::new(deps)` when tests need it), `Result` propagated to the transport
   boundary. Proc-macro `impl` blocks (`#[dataloader]`, `#[hooks]`, trait impls)
   are normal — not an excuse to hide errors or bypass `Repo` except where this
   file names a deliberate, documented exception (e.g. shutdown hooks).
2. **Nest port (second).** Module/feature folders, decorator names
   (`#[module]`, `#[controller]`, `#[resolver]`, `#[field]`, `#[dataloader]`),
   thin handlers, one `service.rs` per feature (≈ `*.service.ts`), same type name
   across crates (`UsersModule`, `UsersResolver` in `domain` vs `apps/api`).
   Nest explains *where* code lives; Rust explains *how* it is expressed.

**Conflict examples (Rust wins):**

| Nest habit | Rust / nestrs decision |
|------------|-------------------------|
| One `UsersResolver` class with everything | `UsersResolver` in `domain` = `#[field]` only; in `app` = root `#[query]`/`#[mutation]` — orphan rules + exposure split |
| Split `users.service.ts` into many topic files | Optional; **one `service.rs`** holding the whole `UsersService` is fine — do not fragment for aesthetics |
| Return `[]` when the DB fails (looks like “no data”) | **Forbidden** — batch/loader methods return `Result` and surface the error |
| `impl ResponseError` in `domain` for ergonomics | Prefer pure errors in `domain`; HTTP mapping in `http.rs` or the app — orphan rules apply |

## Monorepo layout — apps, domain, framework

A multi-app workspace only earns its keep if the split is **a rule, not a
habit**. Three homes, one mandate each:

- **`crates/nestrs-*` — the framework.** Generic, product-agnostic mechanism
  (container, macros, transports, the authn/authz *machinery*). It never names a
  concrete `Claims`, entity, or policy — it is generic *over* them
  (`JwtStrategy<C>`, `AbilityGuard<F>`).
- **`crates/domain` — the product's functional core.** Every feature as a
  module, and **all business logic, once**: entities, services (the entity's DB
  gateway + logic), the JWT contract (`Claims`/`Role`), the authorization
  **policy** (`AppAbility`), resource-server verification wiring, and the
  relation/field resolvers. Modular *inside* — one submodule per concern
  (`identity/`, `authn/`, `authz/`, `orgs/`, `users/`, `oauth/`…) — so
  "centralized" is never "a junk drawer".
- **`apps/<name>` — thin deployables.** A pure **entry point**: it composes
  `domain` modules, attaches transports, and declares the **endpoints**
  (`#[controller]`, root `#[resolver]` query/mutation, `#[gateway]`) exposing
  chosen features over *this* app's transports. **An endpoint holds no logic — it
  delegates to a domain service.** Strip an app of its `domain` imports and only
  routing remains.

**The dividing rule.** Code lives in `domain` when it is the product's
*contract, policy, or logic* — anything another app of the same kind would
reuse. It stays in an app only when it decides **how that app exposes** the
domain. *Exposure is per-app; logic is shared.* A token issuer's `TokenIssuer` +
OAuth strategy live in `domain`; its `/token` controller stays in `apps/auth` —
another app could mint tokens from the same service without exposing the route.

**The compiler draws part of the line.** A `#[field]` relation resolver expands
to `#[ComplexObject] impl Entity`, and `#[expose]` generates the entity's GraphQL
type — both orphan-bound to the entity's crate. So **exposed entities and
relation/field resolvers live in `domain` with the entity; root resolvers,
controllers, and gateways live in the app.** The boundary is enforced by Rust,
not convention.

**Naming across the split — the crate encodes the layer; the file is always
`module.rs`.** A feature has a `domain` module *and* a thin app module, both
named `<Feature>Module` in `<feature>/module.rs`. The **crate boundary** is the
disambiguator (`domain::orgs::OrgsModule` vs `api::orgs::OrgsModule`) — the same
pattern Nest uses across packages. In an app's `module.rs`, **never `use` the
domain module of the same name**; reference it with a fully-qualified path in
`#[module(imports = [domain::orgs::OrgsModule, …])]` instead. Controllers,
resolvers, gateways, processors, and tools in the app folder share the feature
prefix (`OrgsController`, `OrgsResolver`, …) since the folder supplies it.
`app.rs` then reads `imports = [OrgsModule, UsersModule, …]` like a table of
contents. **Never reach for `use … as` to dodge a name clash.** Default to
promoting a feature into `domain`; an app-only struct is the justified exception
of pure exposure.

## Framework rules (after Rust + Nest priority)

1. **Reach for the macros first.** `#[injectable]`, `#[module]`,
   `#[controller]`, `#[routes]`, the per-verb attributes, and their siblings are
   how application code stays declarative. When you wire a service, a feature
   module, or an endpoint, use them. When a pattern recurs and no macro covers
   it, **write a new decorator macro** rather than hand-roll the boilerplate.
   The macros are the leverage we pay to maintain; spending them is the point.

2. **The developer writes business logic; the framework carries the rest.** The
   cross-cutting, error-prone concerns — **security (authn, authz, row-level
   filtering), transactions, and input conversion/validation** — must be
   *transparent*. A feature that forces the developer to hand-manage any of them
   is a defect in the framework, not the app's problem.

This makes **controllers thin**: a handler holds no business logic and no ad-hoc
conversion — it only wires the layers, each with one home:

- a **Guard** decides access and attaches request context (caller, tenant);
- a **Pipe** converts/validates an input at the edge (stateless, no container);
- a **Bind** extractor resolves an id to its loaded, authorized entity (DB-backed
  edge conversion — what a Pipe can't do);
- a **Service** holds the business logic and is the entity's single DB gateway;
- an **Interceptor** carries cross-cutting work (e.g. wrapping a handler in a
  transaction).

Inline conversion, permission checks, or transaction management in a handler is
drift — push it into the matching layer.

## Macro crate structure

A `proc-macro` crate can export only macros, so each decorator lives in a
companion `*-macros` crate re-exported by its home crate (e.g. `#[controller]`
in `nestrs-http-macros`, re-exported so apps write `nestrs_http::controller`).
Shared token helpers go in `nestrs-codegen`. A `*-macros` crate **must not**
depend on its surface crate — it emits absolute-path tokens resolved at the call
site, so there is no cycle. Macro-generated code always uses absolute paths
(`::nestrs_core::*`, `::poem::*`, `::std::sync::Arc`); never rely on call-site
scope.

## Dependency injection is internal

The Rust DI ecosystem was surveyed; none met our maintenance bar. The container
in `crates/nestrs-core` is ours and stays ours. **Do not propose adopting an
external DI crate.** If ergonomics fall short, extend ours.

## Composition model

- **`App::builder().build().await` runs four phases**, independent of call
  order: *seeds* (runtime values a `main` computes), *collect* (each module
  queues the async factories its import tree owns), *factories* (every queued
  factory is awaited — a seed wins over a module factory of the same type), then
  *register* (providers built last, injecting seeds + factory outputs). `main`
  holds only `App::builder().module::<AppModule>()` (+ transports); everything a
  module needs is declared *in* the module tree. Sync apps keep `App::new`.

- **Providers are singletons** unless `#[injectable(scope = request)]` — a
  per-request factory, built once per request, resolving its deps from the
  singleton root. The model is **one level deep**: request-scoped may inject
  singletons, never the reverse and never another request-scoped. Reach one
  through the request boundary (today **HTTP**: `nestrs_http::Scoped<T>`), never
  a `#[inject]` field. GraphQL/MCP do not bridge the scope yet.

- **Modules compose by type or by configured value.** `#[module(imports =
  [...])]` takes a bare type (a static `Module`) or a call expression like
  `OpenApiModule::for_root(opts)` (a `DynamicModule` configured at its import
  site — Nest's `forRoot`/`forRootAsync`). A `DynamicModule` configures via
  `register` (sync) or `collect` (queues an async factory — a DB pool, a queue
  connection). Configuration is each module's responsibility, declared where it
  is imported, never seeded loosely in `main`. Registration is **idempotent**
  (a diamond import builds once); dynamic imports are not deduplicated.

## Encapsulation is compile-time, plus a boot-time access contract

- **Visibility** is Rust's job: the container is flat (a provider is injectable
  by anyone who can name its type), so a feature hides its impl as
  module-private and exposes a `pub` **trait** bound with `provide_dyn`.
  Consumers inject `Arc<dyn Trait>`, never the impl. This is Nest's
  `exports`/`@Injectable` boundary moved to the type system.

- **The import contract** is enforced at boot by the access graph
  (`crates/nestrs-core/src/access.rs`): `#[module]` records its imports and each
  provider's injected `TypeId`s into an `inventory` registry; `App` walks the
  graph from the root and **fails the boot** (`AccessGraphError`) if a provider
  injects something its module neither owns, imports transitively, nor receives
  as global infrastructure (seeds + factory outputs, the `@Global` analog). It
  governs `#[inject]` fields **and** attribute-bound layers (`#[use_guards]` /
  `#[use_filters]` / `#[use_interceptors]`), which are container-resolved at
  mount. The one deliberate hole, named in `access.rs`: runtime
  `Container::get`/`get_dyn` is an unchecked escape hatch — the contract binds
  the *declarative* surface, not imperative resolution.

## Discovery is struct-level by default

Anything a module wires up implements `Discoverable` and is listed in a flat
`#[module(providers = [...])]`. **Default to one struct per concern**, each with
its own decorator macro emitting the single `impl Discoverable` (`#[cron_job]`,
`#[processor]`, `#[mcp]`, a gateway, …) — so third-party crates extend the
system without touching `nestrs-macros`. **HTTP and GraphQL are the
exceptions**: `#[routes]` orchestrates verb attributes on a controller impl, and
`#[resolver]` orchestrates `#[query]`/`#[mutation]`/`#[field]` on a resolver
impl (async-graphql forces method-level kind). Any *further* method-level
decoration needs a strong written justification.

GraphQL composition is **discovered, not listed**: each `#[resolver]` submits
its query/mutation objects to an `inventory` registry merged into the schema
roots at boot — no central `queries = [...]`. The resolver *struct* is still
listed in `providers` for the access contract only. Batch field-resolver fetches
with `#[dataloader]` (request-scoped loaders) to avoid N+1s.

## Lifecycle hooks

`#[hooks]` on a provider's impl block submits each phase-tagged method
(`#[on_module_init]`, `#[on_application_bootstrap]`, `#[on_module_destroy]`, …)
to the same `inventory` registry; `App::run` drains it per phase, resolving the
provider from the container. Hooks are **per-provider**, run in `(provider,
method)` name order; init failure aborts boot, shutdown is best-effort.

## Request layers: guards, filters, interceptors

A `Guard` borrows the request **mutably** — it gates access (return
`Err(Response)`) and may attach request context a handler reads back with
`nestrs_http::Ctx<T>`. Bind guards/filters/interceptors three ways — **global**
(imperative), **controller** (on the struct), or **handler** (beside the verb) —
each container-resolved, no import, first listed outermost. Per-route order,
inner→outer: **shaper → interceptors → guards → filters → meta**. Declarative
per-handler metadata a guard reads ships as `#[meta(EXPR)]` +
`nestrs_http::Reflector`. The one asymmetry: **global** interceptors wrap
*outside* the global guards, because the data context must install the
executor/transaction around the guards too.

URI versioning via `#[controller(version = "1")]` mounts the controller under
`/v1` (`version_path` is the single source of truth).

## Authentication is mechanism; authorization is policy

`nestrs-authn` answers *who the caller is*; `nestrs-authz` answers *what they may
do*. They compose at the request boundary: bind `#[use_guards(AuthGuard,
AppAbilityGuard)]` — `AuthGuard` attaches the principal, `AbilityGuard` builds
the caller's `Ability`. Both the verification alias and the policy live in
`crates/domain` (`authn/`, `authz/`); apps only mount them on their endpoints.

A **`Strategy`** turns a request into a principal (a plain `#[injectable]`, no
macro). **`AuthGuard<S>`** is generic over it. `Strategy::authenticate` returns
an **`Outcome`**: `Authenticated` or `Challenge` (a 401, or an OAuth redirect) —
so one trait serves bearer and OAuth. The standard resource-server case needs no
app strategy: `JwtStrategy<C>` ships it, so `domain` writes the alias once
(`type AuthGuard = AuthGuard<JwtStrategy<Claims>>`) and every resource-server app
mounts it. **`JwtService`** is global infrastructure
(factory phase); it takes a symmetric secret or an EdDSA key pair — a resource
server holds **only the public key** (cannot mint tokens), which is why **token
issuance is its own app** (`apps/auth` signs with the private key; `apps/api` is
a pure resource server that only verifies). The two share `crates/domain` (the
`identity` contract lives there) and the DB, never RPC each other.

### Authz — domain policy vs app transport wiring

Authorization splits like every other feature: **policy in `domain`, transport
bridges in the app that needs them.**

| Home | Owns | Must not |
|------|------|----------|
| `crates/domain/src/authz/` | `AppAbility` (rules), `AppAbilityGuard` (HTTP `AbilityGuard` alias), `AuthzModule` | Import `nestrs-authz-graphql`, `nestrs-authz-ws`, `nestrs-graphql`, or `nestrs-ws` |
| `apps/api/src/authz/` | One **`module.rs`** (`AuthzModule`) + **`bridge.rs`** (`ApiGraphqlGuard` alias) | Redefine policy or duplicate `AppAbility` |

**`domain::authz::AuthzModule`** registers `AppAbility` + `AppAbilityGuard`. REST
controllers bind `AppAbilityGuard` via `#[use_guards]` on the controller (and
feature app modules import `domain::authz::AuthzModule` in their `#[module(imports)]`
when they need the guard in the access graph).

**`api::authz::AuthzModule`** (app — same type name, different crate) imports
`domain::authz::AuthzModule` with an **FQ path** and registers only what this app
needs beyond HTTP:

- `ApiGraphqlGuard` (`GraphqlAbilityBridge<AuthGuard, AppAbilityGuard>`) as
  `dyn OperationGuard`
- `LoaderScope` as `dyn BatchContext`
- `WsDataContext` as `dyn SocketContext`

`app.rs` lists **`crate::authz::AuthzModule` once** — do not also import
`domain::authz::AuthzModule` at the root unless a module in the tree does not
already reach it (today `UsersModule` / `OrgsModule` still import the domain
module for their controllers; the app `AuthzModule` covers GraphQL/WS).

**Anti-patterns (do not reintroduce):** `AuthzGraphqlModule` /
`AuthzWsModule`, files named `graphql_module.rs` or `ws_module.rs` under a
feature folder (`module.rs` is the only DI module file), or a third guard type
that sounds app-specific but is only a GraphQL bridge alias — use `bridge.rs`
and a name like `ApiGraphqlGuard`.

Apps without GraphQL/WS (e.g. `auth`) ship **no** `authz/` folder under the app;
they use `domain::authz` on HTTP routes only.

## The data layer makes security and transactions transparent

The hardest promise — no hand-written row filter, no hand-written transaction —
is kept by a **request-scoped data context** held in two `task_local!`s (a
singleton service has no other way to read per-request state):

- the **executor** (`nestrs-orm`'s `Executor` enum: pool or transaction);
- the **ability** (`nestrs-authz`'s ambient `Arc<Ability>`).

**Hard invariant: every data access goes through a service, and a service
reaches the DB only through `Repo`.** The service (`CrudService`) is the
entity's API and the single audited choke point — controllers, resolvers,
gateways, and dataloaders **delegate to it, never touch `Repo` or the ORM
directly** (resolver/gateway code — not the service methods that implement batch
loads). `CrudService`'s `list`/`page`/`access`/`create`/`update`/`delete`
each go through `Repo` and emit a `nestrs::orm` span (denials at `warn`). `Repo`
runs every query against the ambient executor (joining the request's
transaction with nothing threaded) and filters reads **and** by-id writes by
`condition_for` from the ambient ability — so a feature cannot forget to scope
what the caller may touch (no ability ⇒ `TRUE`, unscoped). By-id route-model
binding goes through the gateway too (`Bind`/`bind` delegate to
`CrudService::access`).

The two task-locals install at different depths: the **executor** outermost via
the auto-registered `DbContext` interceptor (just import `DatabaseModule`) — a
safe method runs on the pool, a mutating one in a transaction committed on
2xx/3xx and rolled back otherwise; the **ability** inside the per-route guards,
via the `#[routes]` **shaper** (the only seam that runs after `AbilityGuard` and
still wraps the handler) — keeping `nestrs-http` unaware of authz/ORM.

**HTTP response masking (`nestrs-authz-http` / `Authorize`).** After a successful
handler, the shaper parses the JSON body, runs `Ability::mask` / `mask_many` on
`Entity::Model`, and re-serializes. Handlers should return the **`#[expose]`
output type** (e.g. `Json<User>`), not `Model` — Uuid fields are often `String`
on the wire. The shaper therefore: (1) parses the wire `Value`, (2) builds
`Model` via `wire_to_model` (filling columns the DTO omits, today `role` and
`password_hash` when deserialization fails), (3) masks, (4) **`retain_wire_keys`**
so an unrestricted field grant cannot leak `#[expose(skip)]` columns (e.g.
`password_hash`). A body that cannot be reconciled with `Model` fails **closed**
with `500`, not unmasked data. **Debt:** teach `#[expose]` to emit wire↔model
metadata so `wire_to_model` does not hard-code column names in the framework.

Two HTTP extractors hand the handler a ready argument: **`Bind<S, A>`** (parse id
→ load + authorize through the service: 404 absent, 403 denied) and **`Scope<E,
A>`** (the explicit `Condition` for a handler building its own query). A route
using `Bind` must also bind an `AbilityGuard`.

The same transparency extends past HTTP through **symmetric, authz/ORM-agnostic
seams** the surface crates expose and the `nestrs-authz-{http,graphql,ws}`
bridge family implements: GraphQL's `OperationGuard` (→ `GraphqlAbilityBridge`,
re-runs the guard chain on `/graphql` only), `BatchContext` (→ `LoaderScope`,
re-installs the snapshotted ability + a **pool** executor around each off-task
dataloader batch), and WebSocket's `SocketContext` (→ `WsDataContext`, installs
the connection's pool + ability per message — no per-message transaction). The
**worker transports** install a pool executor too via the orm-agnostic
`JobContext` seam (`WorkerDbContext`, auto-bound by `DatabaseModule`) — so a
`#[cron_job]`/`#[processor]` gets an ambient `Repo` with no connection injected
(system work ⇒ no ability ⇒ unscoped, correct). A genuinely contextless path (a
shutdown hook) keeps an injected `Arc<DatabaseConnection>` — the **only**
documented bypass of `Repo` on a provider.

**`#[dataloader]` batch methods** live on the service, use `Repo` like any other
read, and return `Result<HashMap<…>, E>` (or infallible only when the method
cannot fail). Never map a DB error to an empty batch — that reads as success and
violates the Rust-first rule.

`apps/api` is the exemplar (REST + GraphQL + WS, DB + authz); `apps/chat` is the
pure real-time exemplar.

### Feature exemplars in `domain` (copy before inventing)

- **`users/`** — `service.rs` holds `UsersService`, `CrudService`, `#[dataloader]`,
  and credential helpers (no separate `loader.rs` / `credential.rs`). `error.rs`:
  `UserError`, `CredentialError` (`Clone` where DataLoader needs it). `http.rs`:
  maps errors to HTTP in the domain crate (orphan-safe). `resolver.rs`: domain
  `UsersResolver` = relation `#[field]` only; `apps/api/src/users/resolver.rs` =
  root `#[query]` / `#[mutation]`. Batch loaders return `Result<HashMap<_, UserError>>`.
  `UsersService::new(db)` is public for tests. Custom `POST /users` returns
  `Json<User>` (DTO), not `Model`.
- **`oauth/`** — `service.rs` (`TokenIssuer`), `strategy.rs` (OAuth + client-credentials
  `Strategy` + guards; Poem types stay here because `Strategy` is HTTP-shaped).
  `error.rs` + `http.rs` at the boundary. Grant logic tests in `domain/tests/oauth/`.
- **`orgs/`** — standard `#[crud]`; align with `users/` if the feature grows
  (single `service.rs`, dedicated `error.rs`).

## The surface crates (the code is authoritative on mechanics)

Each realizes the "new concern = new crate + decorator, no `nestrs-macros`
change" claim. Read the crate for how; here is only what was decided:

- **`nestrs-schedule`** — `#[cron_job]` with exactly one of three triggers
  (`every` interval, `cron` expression with optional `tz`, `after` one-shot);
  string literals validated at compile time, presets/timezones at boot (a bad
  value fails the boot naming the job). The `Scheduler` is a `Transport`.
- **`nestrs-queue`** — Redis-backed via `apalis` (the `@nestjs/bullmq` analog):
  durable, distributed, with retries. A `#[processor(queue = "...", concurrency,
  retries)]` is a struct; queues are **identified by name** (stringly-typed, the
  known cost). Producer and consumer are decoupled. The connection is seeded as
  a factory at the root; no apalis types leak to apps.
- **`nestrs-pipes`** — transport-agnostic, **one `Pipe` per file**, stateless
  (`transform(In) -> Result<Out, _>`, never a DI provider). The base set maps
  Nest (`Parse<T>`, `ParseUuid`, `ValidationPipe<T>`, …). HTTP binds them with
  the `Valid<E>` / `Piped<P, E>` extractors. Reusable pipes are framework
  primitives — never define one in an app.
- **`nestrs-openapi`** — import `OpenApiModule`, it self-mounts `GET /api-json`
  (OpenAPI 3.1) + a bundled offline Swagger UI at `GET /api`. The document is
  **composed** from the route table, not listed. Payload schemas come from
  **schemars** (`Json<T>`'s `T: JsonSchema`); `#[api(...)]` enriches an
  operation.
- **`nestrs-ws`** — **not a `Transport`**: a WS upgrade is an HTTP GET, so
  `#[gateway(path = "/ws")]` self-mounts on the existing `HttpTransport`
  (inheriting its port/CORS/TLS). `#[messages]` orchestrates
  `#[subscribe_message]` + `#[on_connect]`/`#[on_disconnect]`; one JSON envelope
  `{event, data}`. Guards bind at two scopes (connection-level `Guard`,
  per-message `MessageGuard`). Per-gateway namespacing via `WsServer<N>`.

## Naming rules — strict

- Apps live under `apps/<name>/`. Not `examples/`, not `services/` — every
  runnable thing, including the `auth` app, lives under `apps/` uniformly.
- **Files are named by their ROLE**, NestJS-style in snake_case; the folder
  supplies the feature prefix (`orgs/service.rs` ≡ `orgs.service.ts`). Canonical:
  `module.rs` (the DI module — `<Feature>Module` in both `domain/<feature>/` and
  an app's `<feature>/` folder; domain imports in the app side use FQ paths),
  `service.rs`,
  `controller.rs`, `resolver.rs`, `tool.rs`, `entity.rs`, `dto.rs`, `guard.rs` /
  `strategy.rs`, `constants.rs`. No dotted variants. **Never put a role's declaration in a topic file** (a module belongs
  in `module.rs`, not `graphql.rs`).
- A **feature** is **one module per feature-folder per side**: its `domain`
  module wires the services/resolvers (a single module across REST + GraphQL,
  not two), and the app's thin module wires that feature's endpoints and imports
  the domain one. Never split one side's wiring into two modules. A **library
  crate** that bundles several composable `DynamicModule`s (e.g. `nestrs-authn`'s
  `AuthnModule` + `OAuth2Module`) gives **each concern its own feature-folder**
  with one `module.rs` per concern — see `crates/nestrs-authn/src/{jwt,oauth,passport}/`.
  `lib.rs` re-exports the flat surface; it never holds DI wiring itself.
- **`mod.rs`/`lib.rs` carry no business logic** — only `//!` doc, `mod`, and
  `pub use`. The one exception: a proc-macro crate's `#[proc_macro*]` entry
  functions (Rust forces them to the crate root) must be **thin delegations** to
  submodules. `mod.rs` is the folder index; `module.rs` is the DI module — never
  merge them.
- **One role → one file** at the *feature* level (`service.rs`, `resolver.rs`,
  `entity.rs`, …). A folder appears when a feature has several roles
  (`domain/users/`, `nestrs-authn/jwt/`). Do **not** split one service into
  `loader.rs` / `credential.rs` unless a second pattern appears twice — keep the
  whole `UsersService` in `service.rs` (extra `impl` blocks for `CrudService`,
  `#[dataloader]`, `#[hooks]` are required by macros, not extra files). A
  single-role crate stays flat (`nestrs-database/config.rs`). The root
  `AppModule` is a file, `app.rs`.
- **Errors of a feature** belong in `error.rs` (or one clearly named module) —
  not scattered enums inside `service.rs`.
- **No `interfaces/` directory** — a trait lives in the file of its concern (or
  `traits.rs`/`types.rs` for a standalone cluster).
- A file exists only if it has real content (a one-line role file is real
  content; this forbids empty placeholders, not small files). No "small crate
  inlines everything" exception — logic always lives in topical files.

## Engineering posture

- No premature abstraction. Extract after a pattern appears twice.
- Strict typing. Enums over string states. Parse at the edge with established
  crates (`validator`, `uuid` v7). Reserve newtypes for *meaning*, not format.
  Avoid `Box<dyn Any>` / `serde_json::Value` passthrough unless genuinely
  unstructured.
- Errors at boundaries: `thiserror` in libraries, `anyhow` at the app entry. No
  `unwrap()` on production paths. Propagate — do not log-and-pretend-success on
  data paths (especially dataloaders).
- Doc comments only where the *why* is non-obvious; never paraphrase the name.
- **Security is primordial**: access denials and security events log at `warn`+
  (visible in prod), not `debug`.

## Dependency bar

Every new third-party crate must have a published release within the last ~12
months. If a candidate fails this bar, **flag it explicitly** in the proposal.
Never add a stale dependency silently.

## "Done" means verified live

Wiring bugs do not surface in unit tests. Every app ships one `tests/e2e.rs`
booting its real `AppModule` in-process against the live devcontainer
Postgres/Redis — add or extend it. For HTTP/GraphQL changes that is still not
enough: run the binary, `curl` the affected endpoints, then **kill the server
before returning control**.

- **No mocking the database in e2e tests** — real Postgres (testcontainers in
  CI). Unit tests of pure logic need no DB.
- **Three test homes** (do not mix them):
  - **App e2e** — exactly one `apps/<app>/tests/e2e.rs`: boots the real
    `AppModule`, Postgres/Redis, routes and DI wiring.
  - **Crate integration** — layout below; tests the crate's **public** API in
    process, no app boot, no DB unless the crate truly owns persistence.
  - **Cross-crate harness** — `nestrs-testing/tests/<behaviour>.rs` for
    framework wiring shared across crates.
- **Crate integration layout (strict mirror)** — exemplar: `nestrs-authn`.
  - **One binary only**: `tests/<short>.rs` (e.g. `tests/authn.rs` for
    `nestrs-authn`). Every other path under `tests/` is a **module**, never a
    second `tests/*.rs` at the top level (Cargo would compile each as its own
    crate).
  - **Path rule**: for every `src/<path>.rs` with testable logic, maintain
    `tests/<path>.rs` or `tests/<path>/mod.rs` at the same relative path.
    Root-level sources use a directory module (`src/error.rs` →
    `tests/error/mod.rs`, declared `mod error;` in the entry file).
  - **`tests/<feature>/mod.rs`** mirrors `src/<feature>/mod.rs` and declares
    the same submodule names.
  - **The only allowed non-mirror path**: `tests/common/` for shared helpers
    (HTTP fixtures, dev keys, …). Document it in the entry file; never hide
    product logic there.
  - **Documented gaps** (no test file required; say so in the entry `//!`):
    `*/module.rs` (DI/`#[module]` wiring — covered by app e2e),
    `lib.rs` / pure re-export `mod.rs`, trait-only files with no runtime logic
    (may ship an empty `tests/.../strategy.rs` with a pointer to where the
    trait is exercised).
  - **Unit vs integration in a crate**: default to integration tests in the
    mirror tree. Use `#[cfg(test)] mod tests` inside `src/` only when the
    assertion needs private items; prefer refactoring to a testable public
    constructor (`Type::new(deps)`) over container boot.
  - **Testability rule**: if a type is hard to integration-test, fix the API
    (explicit `new`, no leaked secrets in HTTP bodies) — do not skip coverage.
  - **Older crates** may still use flat `tests/<behaviour>.rs`; new work and
    refactors move to the strict mirror.
- A GraphQL app commits its SDL (`apps/<app>/schema.graphql`), regenerated as a
  side effect of the **dev run** (`emit_sdl` driven from the environment) — there
  is no standalone generator and no CI drift-check.

## Hard "no" list

- No external DI library.
- No renaming of `apps/`.
- No feature flags for capabilities that do not yet exist.
- No backwards-compatibility shims (no public API to preserve yet).
- No mocking the database in e2e tests.
- Multiple deployable apps split by responsibility are a goal (not microservices
  sprawl) under two conditions: apps share code through **crates** (never
  copy-paste — all product logic, contracts, and policy live in `crates/domain`;
  see *Monorepo layout*), and the coupling stays **loose** (a self-contained
  token + the shared DB, never chatty RPC).

## Continuity — picking up after a new chat

This file plus the **code** are the source of truth. Past agent transcripts are
not injected automatically; one line of user context (priority + area) is enough
if something is not committed yet.

**Always load:** `CLAUDE.md`, then the feature's `domain` and `apps/<app>/` trees.
**User rules** in Cursor (e.g. explain in French, code/comments in English) apply
per session if configured in the IDE.

**Verified baseline (re-run after authz/HTTP changes):**

- `NESTRS_DATABASE__URL=postgres://nestrs:nestrs@postgres:5432/nestrs cargo test -p api --test e2e` (14 tests)
- `cargo test -p auth --test e2e` (10 tests)
- `cargo test -p domain`

**Open work (intentional gaps — not blockers for unrelated tasks):**

| Area | What |
|------|------|
| `nestrs-authz-http` | Integration tests for `shape.rs` (DTO wire → mask → no `password_hash` leak) |
| `#[expose]` | Replace hard-coded `wire_to_model` defaults with macro-emitted wire↔`Model` bridge |
| `domain` tests | DB-backed tests for `users` `#[dataloader]` batches; Poem-heavy `oauth/strategy` stays e2e/documented gap |
| `orgs/` | Optional alignment with `users/` file layout when touched |
| Live check | `cargo run -p api`, `curl` affected routes, kill server (see *Done* below) |

## Workflow

State the plan in one or two sentences before invoking tools. Batch independent
tool calls in parallel. Run `cargo test --workspace` after meaningful changes;
verify live for routing changes. Kill any background server before returning
control. Report what changed and what was verified — no paragraph-long summary.

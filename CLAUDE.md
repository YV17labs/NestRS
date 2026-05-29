# CLAUDE.md — nestrs

For an LLM picking up this repository. The codebase tells you what *is*; this
file tells you what was decided and what must be respected.

This file is committed to a public repository. Do not introduce machine-local
paths, references to private memory, or links to personal resources.

## What this project is

nestrs is an opinionated Rust framework whose central bet is procedural macros.
Crates under `crates/` provide the building blocks (IoC container, module
trait, the decorator-style macros). Binaries under `apps/<name>/` are real
applications that consume those crates.

NestJS inspired the surface; it is no longer the reference. Describe features
by what they do, not by reflex-pointing at a Nest equivalent.

## The rule that shapes every change

**Reach for the macros first.** `#[injectable]`, `#[module]`, `#[controller]`,
`#[routes]`, and the per-verb attributes are how application code stays
declarative. When you wire a new service, a feature module, or an HTTP
endpoint, use them. When a pattern recurs and no macro covers it, **write a
new decorator macro** rather than duplicate the boilerplate by hand.

Because a `proc-macro` crate can export only macros, each decorator lives in a
companion `*-macros` crate re-exported by its home crate: the
surface-agnostic `#[injectable]`/`#[module]` in `nestrs-macros` (re-exported
by `nestrs-core`); a surface's decorator in that surface's `*-macros` crate
(`#[controller]`/`#[routes]`/`#[interceptor]` in `nestrs-http-macros`,
`#[resolver]` (with method-level `#[query]`/`#[mutation]`) in
`nestrs-graphql-macros`, `#[mcp]` in
`nestrs-mcp-macros`), re-exported by the surface so apps import it from there
(`nestrs_http::controller`). Shared token-building helpers go in
`nestrs-codegen`; a `*-macros` crate must not depend on its surface
crate (it emits absolute-path tokens resolved at the call site), so there is
no cycle. The macros are the leverage the project pays to maintain; spending
them aggressively is the point.

## The developer writes business logic; the framework carries the rest

The project's reason to exist: a developer concentrates on their domain, and the
framework makes the cross-cutting, error-prone concerns — **security
(authentication, authorization, row-level filtering), transactions, and the
conversion/validation of inputs** — *transparent*. A feature that forces the
developer to hand-manage any of those is a defect in the framework's ergonomics,
not the app's problem to solve.

This makes **controllers thin**: a handler holds no business logic and no ad-hoc
conversion. Each concern has one home, and a handler only wires them together:

- a **Guard** decides access and attaches request context (the caller, a tenant);
- a **Pipe** converts and validates an input *at the edge* — the target is to hand
  the handler a domain object, not a raw scalar (resolving an `id` to its loaded
  entity is the same edge conversion; but because a `Pipe` is stateless with no
  container access, that DB-backed binding is the `Bind` extractor, not a `Pipe`
  — see *The data layer makes security and transactions transparent* below);
- a **Service** holds the business logic;
- an **Interceptor** carries cross-cutting work (e.g. wrapping a handler in a DB
  transaction) so the developer never writes it by hand.

If a handler is doing conversion, a permission check, or transaction management
inline, that is drift — push it into the matching layer above. The three concerns
that took the project beyond NestJS — route-model binding, automatic row-level
filtering, and implicit transactions — now ship; see *The data layer makes
security and transactions transparent* below.

## Dependency injection is internal

The Rust DI ecosystem was surveyed; none of the active candidates met our
maintenance bar. The container in `crates/nestrs-core` is ours and stays ours.
**Do not propose adopting an external DI crate.** If ergonomics fall short,
extend ours.

## `App::builder().build()` runs four phases

`Module::register` is synchronous, so a `#[module]` cannot `await` a provider
into existence. `App::builder().build().await` bridges that with four phases,
independent of call order:

1. **Seeds** — `provide`/`provide_arc`/`provide_dyn`: runtime values a `main`
   computes (a loaded config, parsed CLI args) for the DI graph to inject.
2. **Collect** — each module's generated `Module::collect` runs, queuing the
   async factories its import tree *owns* (see the next section). No provider is
   built yet.
3. **Factories** — every queued factory is `await`ed (each sees the container so
   far, so it may depend on a seed or an earlier factory; an `Err` aborts boot).
   A root-level `provide_factory` lands here too, for a resource no module owns.
   A factory whose output type a **seed already supplies is skipped** — a seed
   wins over a module's `for_root` factory. In production nothing seeds a type a
   module factory owns; the path exists so a test (`nestrs-testing`'s
   `EphemeralDatabase`) can seed a pre-built resource in place of the one the
   module would construct.
4. **Register** — each module's `Module::register` builds its providers last,
   injecting the seeds and factory outputs above.

`main` should hold only `App::builder().module::<AppModule>()` (+ transports):
everything a module needs, including async resources, is declared *in* the
module tree, not in `main`. Apps needing no async/runtime values keep using
`App::new` (fully sync — async modules require the `build().await` path).
Registering the same concrete type twice logs a `nestrs::container` warning (the
flat container makes such collisions silent otherwise); a `provide_dyn`
rebinding is exempt — last-binding-wins is its documented override path.

## Providers are singletons unless scoped to the request

The container is a singleton store by default. The one opt-in exception is
`#[injectable(scope = request)]` (the NestJS `Scope.REQUEST` analog): instead of
one shared value, the macro registers a per-request **factory**
(`ContainerBuilder::provide_scoped`) rather than a singleton, and a
`RequestScope` builds the provider **once per request**, caching the instance for
that request. Like a controller it builds lazily, so its register-phase
`Discoverable::dependencies` are empty (it never gates singleton ordering) while
`injected` still reports its `#[inject]` keys, so the access-graph governs it
exactly as it governs an eager provider. A request-scoped provider resolves its
dependencies from the singleton root, so the model is **one level deep**: it may
inject singletons, but not other request-scoped providers, and a singleton may
not inject a request-scoped one (the singleton is built before any request
exists — doing so panics at boot with the missing-provider diagnostic). Reach a
request-scoped provider through the request boundary, never a `#[inject]` field
on a singleton.

The request boundary today is **HTTP**: `HttpTransport` wraps the route tree in a
`RequestScopeEndpoint` that installs a fresh `RequestScope` per request, and a
handler reads a provider back with the `nestrs_http::Scoped<T>` extractor (which
also resolves singletons, falling through — but prefer plain `#[inject]` for
those). Two `Scoped<T>` reads in one request share the cached instance. GraphQL
and MCP do not yet bridge the scope (they carry per-request state through their
own context / DataLoaders); a `transient` scope (fresh instance per resolution)
is likewise not built.

## Modules compose by type or by configured value

`#[module(imports = [...])]` accepts two forms per entry. A bare **type**
(`UsersModule`) is a static `Module`, composed via its associated `register`. A
**call expression** (`OpenApiModule::for_root(opts)`) evaluates to a
`DynamicModule` value — this is how a module receives configuration *at its
import site* (NestJS's `forRoot`/`forFeature`/`forRootAsync`). The macro can't
tell the two `for_root` flavours apart by syntax, so a `DynamicModule` has two
phase methods, both defaulting to no-op:

- `register(self, ContainerBuilder)` — **sync config**. Installs providers /
  metadata immediately (NestJS's `forRoot`). `GraphqlModule`/`OpenApiModule` use
  this for paths, titles, playground toggles.
- `collect(&self, ContainerBuilder)` — **async resource** (NestJS's
  `forRootAsync`). Queues a `provide_factory` for a value that must be `await`ed
  — a DB pool, a queue connection. `nestrs-orm`'s `DatabaseModule` and
  `nestrs-queue`'s `QueueModule` use this. The factory is run in the build's
  factory phase, before any provider is built, so a service can inject the pool.

The macro generates a `Module::collect` that recurses imports (calling
`DynamicModule::collect` on each), letting an async module sit in `imports`
exactly like a sync one — `build()` resolves the ordering. Configuration is
therefore each module's own responsibility, declared where it is imported, never
seeded loosely in `main`. A module also keeps an `impl Module` for its
no-config default, so `imports = [OpenApiModule]` (bare type) still works.

Module registration is **idempotent**: the macro-generated `Module::register`
marks the module's `TypeId` via `ContainerBuilder::mark_registered` and returns
early if already registered, so a module pulled in through several paths (a
diamond) builds its providers exactly once — no double construction, no override
warning. Dynamic-module imports are deliberately *not* deduplicated: each carries
its own config, mirroring `forFeature` being called once per feature.
**Encapsulation is compile-time, not a runtime `exports` list** (see the next
section): `imports` registers providers and orders config, and is a
build-time-enforced *access graph* (a provider may inject only what its module's
transitive imports provide, plus global infrastructure). It still does not gate
*visibility* (what a module can name); that stays the compiler's job.

## Encapsulation is compile-time, via Rust visibility

A module's encapsulation boundary is its Rust `mod`/crate boundary, enforced by
the compiler — there is no runtime `exports`. The container is flat: a registered
provider is injectable by anyone who can **name its type**. So a feature hides its
internals the Rust way — the concrete provider is module-private
(`pub(in crate::feature)` in an app, `pub(crate)`/private in a library crate),
still registered by `TypeId` but unnameable, thus uninjectable, from outside — and
exposes its surface as a `pub` **trait** bound with `Provider as dyn Trait`
(`provide_dyn`). Consumers inject `Arc<dyn Trait>`, never the impl. This is
NestJS's `exports`/`@Injectable` boundary moved from runtime tokens to the type
system: stronger, zero-cost, idiomatic — NestJS needs a runtime boundary only
because TS has no token-level privacy. The exemplar is `apps/mcp/src/weather`
(`WeatherProvider` trait + private `OpenMeteoClient` impl). That covers axis 1
(visibility). **Axis 2 — the import contract — is enforced at boot** by the
access-graph check (`crates/nestrs-core/src/access.rs`):
`#[module]` submits a `ModuleDescriptor` to the link-time `inventory` registry
recording its statically-typed imports and, per provider, its key + injected
`#[inject]` `TypeId`s; `App` walks the graph from the root module(s) and fails
the boot with an `AccessGraphError` if a provider injects something its module
neither owns, imports transitively, nor receives as **global** infrastructure
(seeds + factory outputs — everything present before the register phase, the
`@Global()` analog). The injection set comes from `Discoverable::injected`, kept
**distinct from `Discoverable::dependencies`**: `dependencies` gates the
register-phase ordering and is empty for a provider built later from the
assembled container (a controller, MCP tool, cron job, processor), while
`injected` reports the `#[inject]` keys regardless of build timing — so the
contract governs transport-built logic too. **Every provider in `providers =
[...]` is checked** (`#[injectable]`, `#[interceptor]`, guards, `#[cron_job]`,
`#[processor]`, `#[controller]`, `#[mcp]`); the lone exception is `#[resolver]`,
which self-composes via the GraphQL registry and belongs to no module. Dynamic
(`for_root`) imports are not graph edges: they contribute only global infra or
self-mounted metadata. The contract has **two deliberate boundaries**, named in
`access.rs` so they don't read as total coverage: `#[resolver]` injection is
unchecked (resolvers belong to no module — keep them thin, delegate to
module-registered services that *are* checked), and runtime `Container::get`/
`get_dyn` is an unchecked escape hatch (the `ModuleRef.get()` analog — a flat
container resolves by `TypeId` with no caller identity). The contract binds the
*declarative* `#[inject]` surface, not imperative resolution.

## Discovery is struct-level by default

Anything a module wires up — providers, controllers, interceptors, scheduled
jobs, MCP tools, future event handlers, … — implements `Discoverable` and is
declared in a single flat `#[module(providers = [...])]` list. The container
indexes attached metadata by type; transports and applicative scanners read
it via `DiscoveryService::meta::<MetaT>()`. The `#[module]` macro itself is
generic — it knows nothing about HTTP, MCP, or any specific surface.

**Default to one struct per concern.** A cron job is a struct, an event
handler is a struct, an MCP tool is a struct. Each carries its own
decorator macro (`#[cron_job]`, `#[event_handler]`, `#[mcp_tool]`, …) that
emits the single `impl Discoverable for Self` — no conflict, no central
registry to update, third-party crates extend the system without touching
`nestrs-macros`. **HTTP and GraphQL are the exceptions**: `#[routes]`
orchestrates verb attributes (`#[get]`, `#[post]`, …) on a controller's impl
block, and `#[resolver]` orchestrates `#[query]`/`#[mutation]`/`#[field]` on a
resolver's impl block — because regrouping endpoints (or splitting one
resolver's queries, mutations, and field resolvers) into a struct each would be
absurd. async-graphql forces the split: `#[Object]` makes an entire impl one
root, so method-level kind is the only way to keep a feature's resolver in one
struct. `#[field]` (the field-resolver verb, NestJS's `@ResolveField`) is the
third member of that set, justified by the same logic: a feature's root
queries and the computed/related fields of its types belong in one resolver,
exactly as a NestJS `@Resolver(() => T)` class holds both. Its parameters mirror
NestJS's `@Parent`/`@Args`/`@Loader`: the first, `parent: &T`, is the resolved
object; owned parameters are GraphQL arguments; `&`-reference parameters (a
service, a `DataLoader<…>`) are injected from the container — unambiguous since a
`&T` can never be a GraphQL `InputType`. The macro emits one
`#[ComplexObject] impl T` per parent type, building the resolver from the
container in the GraphQL context. Because async-graphql allows a single
`ComplexObject` per type, a type's fields are owned by one resolver, and `T`
must derive `#[graphql(complex)]`. Any *further* method-level decoration needs
a strong justification and a written design note.

Batch field-resolver fetches with `#[dataloader]` to avoid N+1s. It is an
impl-block decorator on the **data layer** (the service — where the future ORM
query will live), not a loose loader struct: each method
`async fn name(&self, keys: &[K]) -> HashMap<K, V>` (optionally `Result<…, E>`)
generates a hidden `Loader` named `<Owner><Name>` (e.g. `UsersServiceByName`)
wrapping `Arc<Owner>` and delegating to the method — no `#[module(providers =
[...])]` entry. The loaders are **request-scoped, like NestJS**: a schema
extension installed by `GraphqlModule` rebuilds every discovered loader from the
fully assembled container at the start of each request and seeds it into the
GraphQL context, where a `#[field]` reads it back as `&DataLoader<UsersServiceByName>`.
Concurrent `load_one` calls within one request collapse into a single
`Loader::load` (killing the N+1); the per-request instance means no leakage
across requests and lets a loader observe per-request state. Because the loader
is built when a request arrives — not at module registration — `GraphqlModule`'s
import order relative to the data modules it loads is irrelevant, preserving the
project's order-independence guarantee. (A `#[field]` distinguishes the two
injection scopes by type: a `&DataLoader<…>` comes from the request context, any
other `&Service` from the container.)

GraphQL composition is **discovered, not listed**. Each `#[resolver]` impl
submits its generated query/mutation objects to a link-time `inventory`
registry; the schema's roots (`DiscoveredQuery`/`DiscoveredMutation`) merge
their fields from that registry at boot, so there is no central `queries =
[...]` list. This works *despite* async-graphql's static `Schema<Q, M, S>`:
the roots are concrete types whose `create_type_info` reads the registry (via
`Registry::create_fake_output_type`) and whose `is_empty` reports emptiness at
runtime. Import `GraphqlModule` to self-mount the schema at `/graphql`. The
cost is a reliance on async-graphql's public-but-internal `registry` API —
guarded by compile errors and tests when it shifts. Field resolvers are the
exception to this runtime merge: `#[field]` lowers to async-graphql's native
`#[ComplexObject]`, so its fields attach to their type statically — no
registry, no roots.

## Lifecycle hooks self-register through the same registry

The application lifecycle events (NestJS's `onModuleInit`,
`onApplicationBootstrap`, `onModuleDestroy`, `beforeApplicationShutdown`,
`onApplicationShutdown`) attach to a provider that already owns its single
`impl Discoverable`, and a type gets exactly one — so they **cannot** ride the
discovery-metadata path. Instead `#[hooks]` on a provider's impl block submits
each phase-tagged method (`#[on_module_init]`, …) to the same link-time
`inventory` registry GraphQL composition uses; `App::run` drains it per phase,
resolving the provider from the container (the instance request handlers share)
and awaiting the hook. Hooks are therefore **per-provider, not per-module**, and
run in `(provider, method)` name order within a phase — link order is unstable,
and cross-provider init ordering is not expressed (a hook needing another
service injects it). Init phases run after `configure`, before serving, and a
failure aborts boot; shutdown phases run after the transports stop, best-effort.

## Guards bind per route and carry request context

A `Guard` (`nestrs-middleware`) borrows the request **mutably** — it both gates
access (return `Err(Response)` to short-circuit, typically 401/403) and may
*attach* request-scoped context (the authenticated caller, a tenant), the
equivalent of NestJS setting `request.user`. A handler reads it back with the
`nestrs_http::Ctx<T>` extractor (a missing context is a 500 — the guard that
should have set it never ran). Bind guards per route with
`#[use_guards(GuardA, GuardB)]` beside the verb attribute: each is resolved from
the container (so a guard is an `#[injectable]` provider that can inject its own
deps) and the first listed runs outermost. `#[routes]` wraps the handler's
endpoint with them; consumed like the verb attributes, needing no import. Global
guards still attach imperatively via `HttpTransport::guard`. Declarative
per-handler *metadata* a guard reads to vary behaviour (NestJS's
`@Roles`/`Reflector`) ships as `#[meta(EXPR)]` + `nestrs_http::Reflector` (see
the access-graph note above). Two siblings consumed the same way (per-route,
container-resolved, no import): `#[use_filters(FilterA, FilterB)]` binds
exception filters to a single handler (the `@UseFilters` analog —
`HttpTransport::filter` stays the global form) wrapping *outside* its guards;
and URI **API versioning** via `#[controller(version = "1")]` mounts the whole
controller under `/v1` (`version_path` is the single source of truth so the
served path, the boot log, and the OpenAPI document never drift). Header /
media-type versioning is not yet built.

## Authentication establishes identity; authorization consumes it

`nestrs-auth` answers *who the caller is*, the counterpart to `nestrs-authz`'s
*what they may do*. The two compose at the request boundary: an `AuthGuard` runs
first and attaches the principal to the request extensions; an `AbilityGuard`
(`nestrs-authz-http`) then reads that principal to build the caller's `Ability`.
Bind them in order — `#[use_guards(AuthGuard, AppAbilityGuard)]`.

A **`Strategy`** turns a request into a principal, and is a plain `#[injectable]`
provider implementing one method — no `#[strategy]` macro, because `#[injectable]`
plus the trait is the entire surface. **`AuthGuard<S>`** is generic over that
strategy (resolved from the container), mirroring `AbilityGuard<F>`; an app binds
it through a `type` alias. One trait serves both schemes because
`Strategy::authenticate` returns an **`Outcome`**: `Authenticated(principal)` (the
guard attaches it) or `Challenge(Response)` (the guard short-circuits — a `401`,
or an OAuth `302` to the provider). A bearer/JWT strategy always authenticates or
errors; an OAuth strategy challenges on the initiating request and authenticates
on the callback, exactly like Passport's one guard bound to both routes.

The standard bearer case needs **no app strategy at all**: `nestrs-auth` ships
**`JwtStrategy<C>`** (the `passport-jwt` analog) — a generic `#[injectable]`
strategy that pulls the `Bearer` token, verifies it with `JwtService`, and
authenticates as the decoded claims `C`. It is pure *mechanism* with no business
logic, so a resource server writes only a binding — `type AuthGuard =
AuthGuard<JwtStrategy<Claims>>` over its shared claims type (`apps/api`'s `authn.rs`
is exactly this, ~3 lines). This is the **authn-is-mechanism / authz-is-policy**
split: authentication (extract + verify + attach the caller) is framework-generic;
*authorization* (the role/permission rules) is the app's `AbilityFactory`. An app
needing genuinely custom authentication still writes its own `Strategy` — the trait
is the escape hatch, not the default path.

**`JwtService`** (sign/verify, the `@nestjs/jwt` analog) is configured by
`AuthModule::for_root` and provided through the **factory phase**, so it is global
infrastructure — injectable by any strategy, guard, or service regardless of
import order, like the database and queue connections. Construction is synchronous;
the async factory is only the channel that makes it global. It takes either a
symmetric HMAC secret (`JwtOptions::new`) or an asymmetric EdDSA key pair: the
authorization server holds the private key (`JwtOptions::eddsa`), a resource server
holds **only the public key** (`JwtOptions::eddsa_verify`, a verify-only service
whose `sign` errors) — so a resource-server compromise cannot mint tokens. That key
isolation is why token issuance is its own app (see below).

**OAuth2** (`OAuth2Client`, the Authorization Code flow + PKCE) keeps the
provider-agnostic plumbing — build the redirect, exchange the code, fetch the
userinfo — in the crate; the app writes the per-provider `Strategy` (endpoints via
`OAuth2Config`, and the userinfo→principal mapping), exactly as Passport leaves the
provider strategy to the app. The CSRF `state` and the PKCE verifier survive the
round-trip in a short JWT signed by `JwtService` and carried as a cookie — no
server-side session, tamper-proof, no new storage. `oauth2`'s re-exported reqwest
drives the exchange, so the crate adds no second HTTP client. The flat container
keys by type, so one app wires one `OAuth2Client` today; multiple providers would
need per-provider newtypes.

Token issuance is its **own app**, `apps/auth` (the exemplar): it owns the OAuth2
endpoints on the standard paths — `POST /token` (RFC 6749 token endpoint, the
`{access_token, token_type, expires_in}` envelope), `GET /authorize`, `GET
/callback` — and signs EdDSA JWTs with the **private** key. `apps/api` is a pure
**resource server**: it holds only the public key, *verifies* the bearer token
(`JwtStrategy` + `AuthGuard`), and issues nothing. The two share the `identity`
crate (the `Claims`/`Role` token contract both must agree on) and the workspace DB,
but never RPC each other — a self-contained JWT is the only link. This is the
responsibility-split-by-app the "Hard no" list endorses.

## The data layer makes security and transactions transparent

The mission's hardest promise — that a developer writes no row-level filter and no
transaction by hand — is kept by one primitive: a **request-scoped data context**
held in two `tokio::task_local!`s, because a singleton service has no other way to
read per-request state (it cannot inject a request-scoped provider — the model is
one level deep). The two task-locals are:

- the **executor** (`nestrs-orm`'s `Executor`: `Pool(Arc<DatabaseConnection>)` or
  `Txn(Arc<DatabaseTransaction>)`). SeaORM's `ConnectionTrait` has generic methods,
  so it is not object-safe; `Executor` is an enum that *implements* it by forwarding
  the five non-generic methods, so one `&Executor` drives any query as either a pool
  or a transaction.
- the **ability** (`nestrs-authz`'s ambient `Arc<Ability>`, read by `current_ability`).

A service queries through **`Repo<E>`** (`nestrs-orm`) instead of holding a
connection: every call runs against the ambient executor (so it joins the request's
transaction with nothing threaded through), and every *read* is filtered by
`condition_for` from the ambient ability (so a feature cannot forget to scope its
reads — with no ambient ability the filter is `TRUE`, i.e. unscoped). `Repo` requires
an ambient executor and errors otherwise; a non-request context (a dataloader's
keyed batch, a shutdown hook) keeps an injected `Arc<DatabaseConnection>` and queries
it directly. Writes take the ambient executor with `Repo::<E>::conn()`.

The two task-locals are installed at **different depths**, dictated by when each
value exists:

- the **executor** is installed *outermost* by the auto-registered `DbContext`
  interceptor (`nestrs-orm`, activated just by importing `DatabaseModule` — its
  `DynamicModule::register` calls the interceptor's `Discoverable::register`, which
  builds in the register phase, after the connection factory). A **safe** method
  (GET/HEAD/OPTIONS/TRACE) runs on the pool; a **mutating** method runs in a
  transaction opened there and committed on a 2xx/3xx response, rolled back
  otherwise (the txn is shared as an `Arc` for the handler's duration, then
  reclaimed with `Arc::try_unwrap` to commit — a lingering reference falls back to
  `Drop`'s rollback). So the executor is available to guards and handlers alike.
- the **ability** is installed *inside the per-route guards* — the only seam that
  runs after `AbilityGuard` built it and still wraps the handler future is the
  `#[routes]` **shaper**. So the `RouteResponseShaper` trait (`nestrs-http`) is
  `capture` + `run(captured, inner)`: `run` *wraps* the handler future, and
  `Authorize`'s impl (`nestrs-authz-http`) uses it to scope `with_ability` around the
  handler (then mask the response). This keeps `nestrs-http` unaware of authz/ORM —
  the wrap lives in `nestrs-authz-http`, which depends on both.

Two HTTP extractors (`nestrs-authz-http`) hand the handler a ready argument:

- **`Bind<E, A>`** — route-model binding: parse the path id (UUID v7 → `400`), load
  the row through the ambient executor (`404` if absent), instance-check the caller's
  ability (`403` if denied — existence is *not* hidden, matching the gate), else hand
  over the loaded `E::Model`. It is an *extractor*, not a `Pipe`, because loading needs
  the DB. A route using `Bind` must also bind an `AbilityGuard` (it reads the ability).
- **`Scope<E, A>`** — the explicit, Tier-1 counterpart to `Repo`'s transparent
  scoping: yields the `condition_for` `Condition` as an argument, for a handler that
  builds its own query.

`apps/api` is the exemplar: `UsersController::get` is one `Bind` parameter, `list`
takes no filter (the ambient ability scopes `Repo::<Users>::all()`), and `create`
runs in the request's transaction with no annotation. Dependency direction:
`nestrs-orm` → `nestrs-authz` (for `condition_for`) and → `nestrs-http` (the
interceptor); `nestrs-authz-http` → `nestrs-orm` (`Bind` loads). Acyclic.

The HTTP `Authorize` shaper installs the ambient ability, so it is **HTTP-only by
default** — but the same transparency extends to GraphQL through the *symmetric*
seam. Just as `nestrs-http` exposes the authz-agnostic `RouteResponseShaper` that
`nestrs-authz-http` implements, `nestrs-graphql` exposes an authz-agnostic
**`OperationGuard`** seam (a per-operation `before` + `around` the `ContextEndpoint`
runs, resolving an optional implementor from the container via `get_dyn`) that
`nestrs-authz-graphql`'s **`GraphqlAbilityBridge<A, G>`** implements. The bridge is
generic over the app's authentication guard `A` and ability guard `G`, so it runs
the *same guard chain* the controllers use (`AuthGuard` + `AbilityGuard`) and then
wraps the operation in `with_ability` — resolver `Repo` reads scope to the caller
exactly like REST. Because the bridge lives **on the GraphQL endpoint** (not a
global interceptor), `/graphql` is guarded without touching the public routes and
non-GraphQL requests pay nothing. The app binds it behind a type alias and a one-line
module (`apps/api`'s `AuthzGraphqlModule`: `GraphqlAbilityBridge<AuthGuard,
AppAbilityGuard> as dyn OperationGuard`), keeping only the actor-forwarding
`ContextSeed` (its principal type is app-specific). Each resolver still gates with
`nestrs_authz_graphql::authorize::<A, S>(ctx)` (no token → no ambient ability →
`FORBIDDEN`). **The one gotcha:** a `#[dataloader]` batch runs
*off the request task*, so the ambient ability (and executor) do **not** reach it —
a field resolver's loader read is unscoped, and a cross-org field must be confined
another way (e.g. `UsersResolver::namesakes` filters to the parent's org, which is
already in the caller's scope). Scoping the loaders themselves is still open (see
the roadmap).

## Scheduling is the first concern proved out as its own crate

`nestrs-schedule` realizes the "new concern = new crate + decorator, no
`nestrs-macros` change" claim above. A scheduled job is a struct: `#[cron_job]`
builds it from the container (its `#[inject]` fields), implements `Scheduled` for
the logic, and emits the single `impl Discoverable` attaching a `CronJobMeta` —
exactly like a controller, so a job is wired by listing it in
`#[module(providers = [...])]`. The `Scheduler` is a `Transport`: it reads the
discovered metas from the *fully assembled* container at `configure` (so a job
injects any provider, import order irrelevant) and runs each on its `Trigger`.

`#[cron_job]` takes exactly one of three mutually-exclusive triggers, mirroring
`@nestjs/schedule`: `every = "30s"` is a fixed interval (the `@Interval` analog,
`ms`/`s`/`m`/`h` suffixes, first run one interval in); `cron = "0 */5 * * * *"`
is a cron expression (the `@Cron` analog) — `croner` parses 5/6/7 fields
(seconds optional), with `CronExpression::EVERY_MINUTE`-style presets matching
NestJS's enum and an optional `tz = "Europe/Paris"` (IANA name, default **UTC**
for host-independence); and `after = "10s"` runs **once** that long after boot
(the `@Timeout` analog). The macro validates a `cron`/`tz` *string literal* at
compile time; a `CronExpression::X` preset (a const path) and every timezone are
parsed at the `Scheduler`'s `configure`, so a malformed value **fails the boot**
naming the job rather than dying on first fire. The cron engine recomputes the
next occurrence after each run (`croner` over `chrono`/`chrono-tz`); a one-shot
idles until shutdown after its single run so it never races the app down. `croner`
cleared the dependency-freshness bar that previously blocked this.

## Queues are Redis-backed via apalis

`nestrs-queue` is the NestJS `@nestjs/bullmq` analog, built on `apalis` +
`apalis-redis` (the 0.7 stable line — 1.0 is still in release-candidate churn,
below our pre-release bar for a foundation dep). A queue is **durable and
distributed** because it lives in Redis: jobs survive a restart and any number
of worker processes share one queue. It is deliberately *not* the `Scheduler` —
that is in-process periodic work; this is persisted job processing with retries.

A consumer is a struct, like a cron job: `#[processor(queue = "welcome-email",
concurrency = 5, retries = 3)]` builds it from the container (`#[inject]`
fields), implements `Processor` (`type Job` + `async fn process`), and emits the
single `impl Discoverable` attaching a `ProcessorMeta` — so a processor is wired
by listing it in `#[module(providers = [...])]`. The `QueueWorker` is a
`Transport`: at `configure` it reads the discovered metas from the *fully
assembled* container and resolves the shared connection; at `serve` it builds one
apalis worker per processor on a single `Monitor` and runs it under
`run_with_signal(cancel)`. `concurrency` maps to the Redis source's fetch buffer
(the in-flight-job ceiling apalis runs as a `FuturesUnordered`), not a worker
count — `register_with_count` is deprecated in 0.7. A processor's `process` error
is boxed and retried up to `retries` by an apalis `RetryLayer` (which is why a
`Job` payload must be `Clone`).

Queues are **identified by name** (a string), matching `@Processor('name')` —
the explicit, NestJS-familiar model chosen over keying by payload type. The
producer injects the shared `QueueConnection` and enqueues by that name:
`self.queue.of::<WelcomeEmail>("welcome-email").push(job).await?`. The name at
the call site must match the consuming `#[processor(queue = "...")]`; this is the
known cost of the stringly-typed model. Producer and consumer are decoupled — a
process may enqueue to a queue whose processor lives elsewhere, so `Queue<T>`
registration is **not** tied to `#[processor]`.

The Redis connection is async, so it is **not** a module provider: seed it once
at the composition root with an `App::builder()` factory
(`provide_factory(|_| QueueConnection::connect(url))`), and the `QueueWorker`
transport plus every producer resolve it from the container — import order
irrelevant, the same final-container contract the `Scheduler` relies on. All
apalis types stay inside `nestrs-queue` (it re-exports nothing apalis-shaped to
apps, mirroring how `nestrs-graphql` wraps async-graphql), so `#[processor]`
emits only `::nestrs_queue::*` paths and an app never takes a direct apalis
dependency. `register_worker::<P>` is the monomorphic thunk the macro stores in
the meta — the queue analog of `#[cron_job]`'s `RunFn`.

## Pipes are a transport-agnostic crate, applied at the surface boundary

NestJS pipes (validation + transformation of a handler parameter) are **not** an
HTTP concern — they live in `nestrs-pipes`, a pure crate with no transport or
container dependency, **one `Pipe` per file**. The `Pipe` trait is stateless
(`transform(In) -> Result<Out, PipeError>`, an associated fn — a pipe is a
zero-sized marker named at a call site, never a DI provider, so no decorator
macro is needed). The base set maps NestJS: `Parse<T>` (+ `ParseInt`/`Float`/
`Bool` aliases, and any `FromStr` enum — covering `ParseEnumPipe`),
`ParseUuid`/`ParseUuidVersion<V>`, `ParseArray<T>`, `Trim`/`Lowercase`/
`Uppercase`, and `ValidationPipe<T>` (runs `validator`).
`DefaultValuePipe`/`ParseFilePipe`/`ParseDatePipe` are intentionally absent —
the crate docs give the Rust-idiomatic replacement for each.

A *surface* binds a pipe to a parameter. HTTP does it with two poem extractors
in `nestrs-http` (the only HTTP-specific part): `Valid<E>` (e.g.
`Valid<Json<T>>`) runs validation, `Piped<P, E>` applies a transform — both
reject with the `PipeError` rendered as a JSON 400 before the handler runs.
Typed extractors (`Path<u32>`) already cover plain parses, so there is no
`ParseIntPipe` extractor. Reusable pipes are framework primitives — never define
one in an app. (Aside: poem rejects two `.at(path, …)` for one path, so
`#[routes]` collapses several verbs on a path into a single `RouteMethod`,
letting a collection controller hold `GET` and `POST /users`.)

## OpenAPI documents itself from the route table

`nestrs-openapi` is the REST analog of `nestrs-graphql`: import `OpenApiModule`
and it self-mounts (via `HttpEndpointMeta`, like `GraphqlModule`) `GET /api-json`
(the OpenAPI 3.1 document) plus a **bundled, offline** Swagger UI at `GET /api` —
the NestJS-convention paths, since the OpenAPI spec mandates none. The
document is **composed, not listed** — it reads the same `HttpControllerMeta`s
the transport mounts, so every `#[controller]` in the binary appears with no
central registry. Swagger UI assets are vendored under
`crates/nestrs-openapi/assets/` (pinned `swagger-ui-dist`) and embedded with
`include_bytes!`; no CDN, no app-facing utoipa/poem-openapi dependency.

Paths and verbs are free from the route table; the missing piece — payload
**types** — is captured in `#[routes]`, the only place they are visible. It
records, per route, a `schema_of::<T>` fn-pointer for any `Json<T>` request body
or response (peeling `Result`/`Valid`/`Piped` wrappers; `Vec`/`Option` are left
to schemars), into `HttpRouteMeta`. The schema layer is **schemars**, not a new
dependency: DTOs derive `schemars::JsonSchema` exactly as they derive async-
graphql's `SimpleObject` for GraphQL or as MCP tool params already do — one
derive per surface. The one new contract: **a `Json<T>` payload's `T` must
implement `JsonSchema`** (handlers returning a raw `Response`/`String` capture no
schema and need no bound). `nestrs-openapi` drives a single 2020-12 generator
(`SchemaSettings::draft2020_12()` with `definitions_path` repointed to
`/components/schemas`) across all routes — 2020-12 *is* the OpenAPI 3.1 schema
dialect, so no `nullable`/single-type rewriting is needed and `$ref`s resolve
under `components/schemas`.

`#[api(summary = "...", description = "...", tags("..."))]` beside a verb
attribute enriches an operation (NestJS's `@ApiOperation`/`@ApiTags`); it is
consumed by `#[routes]` like `#[use_guards]`, needs no import, and every field is
optional — the default tag is the controller struct name, so routes group by
controller. Not yet built (deliberate, noted in the crate): query-param schemas,
path-param *types* (emitted as `string`), security schemes, and a committed
`openapi.json` snapshot emitted on boot like the GraphQL SDL (see "Done" below).

## Naming rules — strict

- Applications live under `apps/<name>/`. Not `examples/`, not `services/`.
  The first was rejected because these are real applications, not samples; the
  second because *every* runnable thing — including a dedicated service like the
  `auth` app — lives under `apps/` uniformly, with no separate `services/` tier.
- **Files are named by their ROLE**, NestJS-style, in Rust snake_case — the
  folder supplies the feature prefix NestJS puts in the filename
  (`orgs/service.rs` ≡ `orgs/orgs.service.ts`). The canonical role files and their
  NestJS analog:

  | Role | File | NestJS |
  |---|---|---|
  | DI module | `module.rs` | `*.module.ts` |
  | service | `service.rs` | `*.service.ts` |
  | HTTP controller | `controller.rs` | `*.controller.ts` |
  | GraphQL resolver | `resolver.rs` | `*.resolver.ts` |
  | SeaORM entity | `entity.rs` | `*.entity.ts` |
  | DTOs | `dto.rs` | `*.dto.ts` |
  | guard / strategy | `guard.rs` / `strategy.rs` | `*.guard.ts` / `*.strategy.ts` |
  | constants | `constants.rs` | `constants.ts` |

  Do not invent dotted variants (not valid Rust module names), and **never put a
  role's declaration in a topically-named file** (a module belongs in `module.rs`,
  not `graphql.rs`).
- **A `#[module]`/module is declared in `module.rs`, never a topic file** (a module
  belongs in `module.rs`, not `graphql.rs`). An app **feature** is *one module per
  feature-folder* — never two modules in one app feature-folder (e.g. a single
  `AuthzModule` wiring REST + GraphQL, not `AuthzModule` + `AuthzGraphqlModule`); the
  surface split stays at the *framework-crate* level. A **library crate** is a
  different unit: it may expose several configuration modules from its own
  `module.rs` (e.g. `nestrs-auth` provides `AuthModule` + `OAuth2Module`, kept
  independently composable so a resource server takes JWT without OAuth).
- **`mod.rs`/`lib.rs` carry no business logic** — only `//!` doc, `mod`
  declarations, and `pub use` re-exports. The one thing besides those they may hold
  is, in a **proc-macro crate**, the `#[proc_macro*]` entry functions: Rust *forces*
  macros to the crate root, so they live in `lib.rs` and must be **thin delegations**
  to submodules (the implementation, helpers, and any `macro_rules!` go in topical
  files). This is not a style exception — it is the only place the language permits a
  proc-macro. `mod.rs` and `module.rs` are therefore different layers: `mod.rs` is
  the Rust folder index (plumbing), `module.rs` is the DI module (content) — never
  merge the `#[module]` into `mod.rs`.
- **One role → one file (no folder).** A single-purpose thing is a single
  `<name>.rs`, no directory and no `mod.rs`. A folder (with `mod.rs` + role files)
  appears **only** when a feature genuinely has several roles — so "many files"
  always means "many roles," never ceremony. The root `AppModule` is a file,
  `app.rs` (the NestJS `app.module.ts` root-as-file equivalent), not a directory.
- **No `interfaces/` directory.** A Rust trait is a first-class type: it lives in
  the file of its concern (a trait beside its abstraction), or a `traits.rs` /
  `types.rs` for a standalone cluster — we take the Rust norm here, not a mirrored
  TS `interfaces/` folder.
- A file exists only if it has real content. No placeholders for symmetry — but a
  one-line role file (e.g. a `guard.rs` holding only a `type` alias) **is** real
  content; this rule forbids empty/placeholder files, not small role files. There is
  **no "small crate inlines everything" exception** — business logic always lives in
  topical files, never `lib.rs`.

## Dependency bar

Every new third-party crate must have a published release within the last
~12 months. If a candidate fails this bar, flag it explicitly in the proposal.
Do not add a stale dependency silently.

## "Done" means verified live

Routing and wiring bugs do not surface in **unit** tests. An **e2e** test
(`nestrs-testing`, in-process — every app has one) now catches most of them in
`cargo test`, so add or extend it. But for HTTP or GraphQL changes that is still
not sufficient: start the binary (`cargo run --bin <app>` in the background),
`curl` the affected endpoints, then kill the server before returning control —
real-socket behaviour and live external services are what the in-process harness
cannot reach.

**Test-file naming (strict).** An **app** has exactly one `tests/e2e.rs` that boots
its `AppModule` end-to-end (`apps/db` has no `AppModule`, so its `e2e.rs` instead
migrates a throwaway database with its own `Migrator` and runs the seed — still one
`e2e.rs`, no exemption). A **crate** names its integration tests by the *behaviour*
they exercise — `tests/<behaviour>.rs`, one scenario per file (`authorize.rs`,
`scheduler.rs`, `resolver_guard.rs`, …) — since a crate has several. Cross-crate
**in-process** integration tests (a feature of `nestrs-http`/`nestrs-core` driven
through the harness) live in `nestrs-testing/tests/`, the crate that owns `TestApp`
(`cors.rs`, `reflector.rs`, `versioning_filters.rs`, …) — so a framework crate need
not take a dev-dependency on the harness.

A GraphQL app commits its schema as SDL (`apps/<app>/schema.graphql`) so the API
surface is reviewable in diffs. The schema is composed from the resolvers
*linked into a binary* and rendered from the fully assembled container, so it can
only be emitted from inside the running app — there is no standalone generator.
`GraphqlModule::for_root` carries this with two plain fields: `schema_path`
(where the SDL lives) and `emit_sdl: bool` (whether to write it). When `emit_sdl`
is `true` the server writes `schema_path` (deterministically — types, fields, and
arguments sorted, so link-time iteration order cannot churn the diff) once at
boot, before serving, from the same container the resolvers inject into. The app
drives the boolean from its environment at the import site (`apps/api` uses
`cfg!(debug_assertions)` — `true` in a dev build, `false` in a release one), so a
dev run regenerates the committed schema while production reads it as-is. A write
failure is logged, never fatal.
Regenerating after changing resolvers therefore means **running the dev server**
— there is no `bin/schema.rs`, no `nestrs-graphql-cli`, no `just graphql-schema`,
and no drift-check: regeneration is a side effect of the dev run, not a CI gate.
(Federation-aware schema commands, if federation lands, would reintroduce
dedicated tooling.)

## Engineering posture

- No premature abstraction. Extract after a pattern appears twice, not before.
- Strict typing. Enums over string-typed states. Parse at the edge using
  established crates (`validator` for declarative input checks, `uuid` for
  UUID v7 IDs) rather than hand-rolling newtypes for every format-validated
  string. Reserve newtypes for values whose *meaning* — not just format —
  needs the type system's help. Avoid `Box<dyn Any>` and `serde_json::Value`
  passthrough unless the data is genuinely unstructured.
- Errors at boundaries: `thiserror` in libraries, `anyhow::Result` at the
  application entry. No `unwrap()` on production paths.
- Doc comments only where the *why* is non-obvious. Never paraphrase the
  type or function name.
- Macro-generated code uses absolute paths (`::nestrs_core::*`, `::poem::*`,
  `::std::sync::Arc`). Never rely on what is in scope at the call site.

## Hard "no" list

- No external DI library.
- No renaming of `apps/`.
- No feature flags for capabilities that do not yet exist.
- No backwards-compatibility shims (no public API to preserve yet).
- No mocking the database in **e2e tests** — exercise a real Postgres (the dev
  container provides one; `testcontainers` in CI). Unit tests of pure logic
  (validation, field mapping) need no database and may use a default connection.
- Multiple deployable apps, **split by responsibility** (and yes, scalability),
  are a goal — e.g. a dedicated `auth` app (`auth.*`) issuing tokens and a pure
  resource `api` (`api.*`) verifying them. What keeps that in scope, not a
  forbidden microservices sprawl, is two conditions: **(1)** apps share code
  through **crates**, never copy-paste (framework crates are `nestrs-*`;
  business-shared contracts like the JWT claims live in a non-`nestrs-` crate such
  as `identity`); and **(2)** the coupling stays **loose** — apps connect through a
  self-contained token + the shared DB, not chatty runtime RPC (`api` verifies a
  JWT locally with the *public* key; it never calls `auth`). Don't split before a
  real boundary exists, and don't fragment without shared libraries.

## Workflow

State the plan in one or two sentences before invoking tools. Batch
independent tool calls in parallel. Run `cargo test --workspace` after
meaningful changes; verify live as above for routing changes. Kill any
background server before returning control. Report what changed and what was
verified — no paragraph-long summary.

## What this file deliberately does not contain

The crate layout, the dependency versions, the macro signatures, the test
counts, the file tree. The code is authoritative on those — read it. This
file only states what the code cannot.

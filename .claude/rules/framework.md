---
paths:
  - "crates/nest-rs-*/**/*.rs"
  - "crates/nest-rs-*/**/*.toml"
---

# Framework crates — macros, container, discovery

Loaded when touching `crates/nest-rs-*`. See also: `request-layers.md`,
`data-layer.md`, `authn-authz.md`.

## Macros

**Reach for macros first.** When wiring a service, module or endpoint,
use the decorators. When a pattern recurs without one, write a new
decorator — if it clears the bar below.

A `proc-macro` crate can only export macros, so each decorator lives in
a companion `*-macros` crate re-exported by its home crate. **That is
the one licensed exception to "`lib.rs` carries no logic"** — Rust
forces `#[proc_macro_attribute]` items to the crate root, so a
`*-macros` `lib.rs` holds them and they stay thin delegations into the
crate's own modules. The rule shipped to products has no such carve-out,
correctly: a generated project has no macro crate. Shared token
helpers in `nest-rs-codegen`. A `*-macros` crate **must not** depend on
its surface crate — emit absolute-path tokens; never rely on call-site
scope. Testable form: **a `*-macros` crate emits only `::std`/`::core`
paths or paths routed through its surface crate's re-exports
(`::nest_rs_<x>::<dep>`) — never a bare third-party path** (`::anyhow`,
`::tracing`, …), which resolves against the *consumer's* extern prelude
and breaks any app lacking that direct dep.

**The root is the umbrella, not the sibling.** A `*-macros` crate emits
`::std`/`::core` paths, or paths rooted at `::nest_rs::<concern>::` —
never `::nest_rs_<sibling>::` directly, and never a bare third-party
path. Non-API seams stay `#[doc(hidden)]` in the crate that owns them;
the root is what carries the contract. The developer declares
`nest-rs` with the capability's feature and nothing else; see *The
umbrella is the front door* in `CLAUDE.md`. Routing through the umbrella
is also what dissolves the cycles — `nest-rs-guards`, `nest-rs-authz`
and `nest-rs-seaorm` sit *above* the transports, so a sibling root can
never reach them, while `::nest_rs::` sits above all of them.

Two exceptions survive, and neither is a licence:

- **Emitted derives** (`::serde`/`::validator`/`::schemars`). A derive's
  own expansion targets the call-site prelude, so re-export routing
  would be false hygiene. The fix is the derive's `crate = ` override
  plus a re-export from the surface crate; until a given derive has it,
  the path is legal **only when the developer's own source writes that
  derive**. Same for the entity-site trio `::sea_orm`/`::uuid`/
  `::chrono`: an entity file names them itself.
- **poem's `#[handler]`**, which `#[routes]`/`#[crud]` wrap and whose
  expansion targets the call-site prelude. This one is a **known defect,
  not a design** — a controller crate should not have to declare `poem`.
  It is reported on `/http/`, never argued away.

"The use site owns that crate by definition" is not an admissible
reason. Owning a capability means enabling its feature.

The proof is compile-time: `nest-rs-macro-hygiene` (workspace,
`publish = false`) consumes decorators with **zero** third-party deps —
extend it when adding a decorator. It holds **decorators only**: a module
import there proves nothing about a macro and squats a proof that belongs
in the owning crate's own suite (see *Shipping a new capability* step 5
in `CLAUDE.md`).

It deliberately does **not** consume `#[crud]`/`#[expose]`: those need a
real entity and a real service, so their contract is proved by
`crates/nest-rs-cli/tests/e2e/scaffold.rs` — which scaffolds a workspace,
generates a resource, repoints `nest-rs` at the working tree through
`[patch.crates-io]`, and runs a real `cargo check` over the result.

**That distinction is load-bearing.** The `integration` suite asserts on
the *text* the generator wrote, which catches a wrong dependency and can
never catch a template that emits code the compiler rejects — only a user
would find that. A template change is not done until the `e2e` suite has
run; it shares the repo's target directory, so a warm run is seconds.

**One naming exception, decided:** every host decorator is named for its
role (`#[controller]` → `controller.rs`, `#[resolver]`, `#[gateway]`,
`#[processor]`) **except `#[mcp]`**, whose file the role table names
`tool.rs`. Deliberate: `nest-rs-mcp` re-exports rmcp's own `#[tool]`, and
the tool host carries both — a `#[tools]` one letter away from the
`#[tool]` beneath it reads as a typo. The role word lives in the file name
and the module instead. **Do not "fix" this by adding a second decorator.**

**`#[mcp]` decorates two item shapes, and that upholds the rule rather than
bending it.** On the struct it declares the host and its endpoint; on the
`impl` it declares the operations, the way `#[routes]` does for a controller.
One name, so nothing sits a letter from `#[tool]`, and nothing new to learn.
The impl form is what absorbs rmcp's three-block shape — `#[tool_router]`,
`#[prompt_router]`, `#[tool_handler]`/`#[prompt_handler]`, `get_info` — into
generated code, and it earns its keep three ways beyond the line count:

- **`use rmcp;` leaves the developer's file.** rmcp's macros resolve bare
  `rmcp::` paths against the call site, so a host had to carry an import whose
  only job was someone else's hygiene. The expansion emits those impls inside a
  private child module that carries the import itself. Two Rust facts make it
  sound and both are asserted in `nest-rs-mcp/tests/integration/mcp_impl.rs`:
  an inherent impl may live in any module of the defining crate (a descendant
  still reaches the parent's private fields), and an item's **own visibility**,
  not the module it sits in, decides who may name it.
- **That second fact is load-bearing, not trivia.** rmcp generates
  `tool_router()` *without* `pub`, so reading it from the parent silently yields
  an empty tool list — the duplicate-tool boot check would go blind. The
  expansion emits its own `pub(crate)` accessor beside it, and
  `DefaultDeclaredTools` is the empty fallback for a host that has no decorated
  impl.
- **Capabilities are derived, never restated.** A `#[tool]` method advertises
  `tools`, a `#[prompt]` method `prompts`. A host can no longer route
  operations it forgot to declare — the defect the CLI template itself shipped.

**The escape hatch is a host that owns its `ServerHandler`.** Resources,
completion and the rest are hand-written trait methods, and the sugar cannot
generate a second `impl ServerHandler`; such a host writes rmcp directly, and
`#[mcp]` on a trait impl is a compile error saying so. `demo`'s `posts` is that
host, deliberately kept as the witness of the raw shape.

### When (not) to write a decorator

**Write one when all three hold:** the pattern appears in ≥ 3 places;
the boilerplate is mechanical; the rule is teachable in one sentence.

**Never for:** business logic; one-off integrations; context-dependent
inference Rust can't give (prefer a builder); anything needing `unsafe`
or runtime reflection.

Ships with: a doc comment showing the expansion; a test in the home
crate's `tests/` (or `nest-rs-testing` for cross-crate wiring); a use
site in an app or `features`. **Compile cost > 0.5 s per use site is a
defect. Measure.**

## The DI container is internal

Surveyed the ecosystem; none met our bar. **Do not propose an external
DI crate.** Extend ours.

### Composition model

- **`App::builder().build().await` runs four phases** independent of
  call order: *seeds* (runtime values from `main`), *collect* (modules
  queue async factories), *factories* (awaited; seed wins over factory
  of same type), *register* (providers built, injecting seeds + factory
  outputs). `main` holds only `App::builder().module::<AppModule>()`
  (+ transports). Sync apps keep `App::new`.
- **Providers are singletons** unless scoped. Two non-default scopes:
  - `#[injectable(scope = request)]` — built per request, deps from the
    singleton root. **One level deep**: request-scoped may inject
    singletons; never the reverse or another request-scoped. Reach one
    through the request boundary (`nest_rs_http::Scoped<T>`,
    `nest_rs_graphql::Scoped<T>`, `nest_rs_mcp::Scoped<T>`), never via
    `#[inject]`.
  - `#[injectable(scope = transient)]` — rebuilt on **every** resolution,
    no caching. May depend on singletons or request-scoped. A transient
    that transitively depends on itself **panics at resolution** with a
    cycle diagnostic naming the chain — the one provider error caught at
    first-resolution rather than at boot. Singleton is the default;
    reach for transient only when a fresh instance per use is genuinely
    required.
- **Modules compose by type or configured value.** `#[module(imports =
  [...])]` takes a bare type or a call like `OpenApiModule::for_root(opts)`
  (`DynamicModule`). Configure via `register` (sync) or `collect` (async
  factory). Registration is **idempotent** (diamond imports build once);
  dynamic imports are **not** deduplicated.

### `for_root` — one seam, one value, no chain

**A module is configured in exactly one place, by exactly one value.**
`Module::for_root(x)`, and `x` carries *everything* the app declares
about that module. The `DynamicModule` it returns is **opaque**: no
public method on a `*Setup`, no second constructor on the module type.
A declaration that does not fit into `x` makes `x` grow a **field** —
never the seam a **method**. A builder chain (`for_root(None).thing(t)`)
is three spellings of one import, and the second constructor added to
soften it (`Module::thing(t)`) is the fourth; both are defects.

Testable form, both halves checkable: **`rg 'impl \w+Setup' crates/`
returns nothing**, and **no module type has an inherent `pub fn` besides
`for_root`** — with `ConfigModule` the single carve-out, because it is
the config crate itself rather than a configurable module. Its
`for_root` / `for_feature` / `provide_feature` / `setup` are the
primitives every other module's seam is *built from*, and
`provide_feature` is public API a third-party driver calls
(`docs/…/database/writing-a-driver.mdx`). Nothing else gets that
exemption: `nest_rs_throttler::provide_guard` / `resolve` are the same
kind of cross-crate seam and they are `#[doc(hidden)]`.

Two shapes for `x`, and only two:

- **`impl Into<Option<C>>`**, `C` being the module's `#[config]` — the
  default, and what almost every module wants. `None` ⇒ env over
  `C::defaults()`; `Some(c)` ⇒ env over `c`, per field.
- **`impl Into<MOptions>`**, where `MOptions { config: Option<C>, /* … */ }`
  is a plain `Default` struct declared beside the setup in `module.rs` —
  only when the module carries a declaration that genuinely has **no env
  twin** (`McpIdentity`). It keeps `From<C>` and `From<Option<C>>` so the
  config-only call site reads exactly like every other module's.

**Don't hand-write the setup when the shape is plain.** A `for_root` whose
whole job is "pin the config, then recurse into my own wiring" returns
`ConfigSetup<M, C>` — `pub type WsSetup = ConfigSetup<WsModule, WsConfig>;`
plus a one-line `for_root` calling `ConfigModule::setup(config)`. Keep the
alias: the name is what the docs and `for_root`'s signature reference. Write
your own type only when `collect` queues more than the config (a pool, a
client) or `register` does more than recurse (`McpSetup`, `HttpSetup`,
`GraphqlSetup`, `OpenApiSetup`). The constructor lives on `ConfigModule`
rather than as `ConfigSetup::new`, so a *shared* setup is as opaque as a
hand-written one and the first half of the testable form still holds.

The `Option` inside `MOptions` is load-bearing, not a habit: `Config::resolve`
ranks the `.env` cascade *below* a pinned base and *above* `defaults()`, so
flattening it to a bare `C` would silently demote the cascade.

**This does not outlaw the bare import.** `imports = [WsModule]` is a
*dependency declaration* — "my providers inject `Arc<WsServer>`" — and it
configures nothing, so it is not a second seam. A module that must not be
imported bare hides its `#[module]` behind a private host struct and
exposes only the plain façade: `ProtectedResourceHost` /
`ProtectedResourceModule` is the exemplar.

**The ownership table — which config reaches which seam — is in
`architecture.md`** (*Configuration — one seam per config*), because a product
developer needs it too and that file is loaded in every session. Read it there.
Its one line for this crate: **every `nest-rs-*` module owning a `#[config]`
owns a `for_root`**, because a consumer cannot edit your `Default` and that seam
is their only in-code path. The obligation stops at the crate boundary — a
product's own module already has `impl Default`. Three points belong here, with
the reasoning the table omits:

**`for_root` configures; `for_feature` registers.** This is NestJS's split, and
it is not stylistic: `forRoot` configures a module once, `forFeature` registers
artifacts against an already-configured one (`TypeOrmModule.forFeature([User])`
mounts repositories, it does not reopen the connection). Our `for_feature` takes
no value for that reason. It briefly took a pinned base, and that was the defect
— it made every module-owned config reachable two ways, which is the *second
seam* this whole section forbids.

**A module that owns no `#[config]` gets no `for_root`** — the sharper half.
`SocialModule` is the case that proves it: a provider carries its own
`#[config]` and the registry entry names it, so discovering the provider is what
loads its credentials. The module never learns which providers exist, so there
is nothing for it to be configured about. Giving it a `for_root` anyway forces a
list of mutually unrelated config types, hence type erasure, hence no duplicate
detection and a hand-written `Debug` to keep a client secret out of the format —
all paying for a declaration the discovery seam already made unnecessary.

**The rule is enforced, not merely written.** A value an import site *chose* is
queued as a **declaration** (`ContainerBuilder::provide_declared_factory`,
which takes the remedy sentence its error will print). Three consequences, each
with a witness in `nest-rs-config`:

- a declaration **supersedes** an ordinary factory for the same type, wherever
  the two fall in `imports = [..]` — `a_pin_survives_a_bare_import_listed_before_it`;
- two declarations for one type raise `ContestedDeclarationError` before any
  factory runs — `two_pinned_bases_for_one_config_fail_the_boot`;
- the synchronous `App::new` refuses a queued factory it could never drain
  (`UnresolvedFactoryError`) — `the_synchronous_boot_refuses_a_config_it_could_never_resolve`.

**It is not config-only.** Any module binding an implementation a *sibling
module also binds* declares it, so importing both is a named boot failure
rather than whichever `imports` listed first: `ThrottlerModule` and
`RedisThrottlerModule` both bind `Arc<dyn ThrottlerStore>`, and they share one
`BACKEND_REMEDY` constant so the two halves cannot drift.

This is where we deliberately exceed NestJS, which lets the last registration
win in silence — a dropped declaration is on the wrong side of *no silent
failure*.

**One recorded exception: `OpenTelemetry::init_with(config)`.** The global
tracer and meter must exist *before* any module registers (the module panics
otherwise) and the returned guard's `Drop` flushes, so it belongs to `main`
and cannot be an import. Any other module claiming an exception is reported,
not written.

### Access contract (compile-time + boot-time)

- **Visibility is Rust's job.** Flat container ⇒ hide impls
  module-private, expose a `pub trait` bound with `provide_dyn`.
  Consumers inject `Arc<dyn Trait>`. **No `exports` list.**
- **Import contract enforced at boot** by the access graph
  (`crates/nest-rs-core/src/access.rs`): `#[module]` records imports and
  each provider's injected `TypeId`s into `inventory`; `App` walks from
  the root and fails boot (`AccessGraphError`) if a provider injects
  something its module doesn't own, import transitively, or receive as
  global infra (seeds + factory outputs). Governs `#[inject]` **and**
  `#[use_guards]`/`#[use_filters]`/`#[use_interceptors]`. Runtime
  `Container::get`/`get_dyn` is an unchecked escape hatch — the contract
  binds the declarative surface only.
- **Single flat container** — no per-module sub-container. Orphan rules
  prevent accidental coupling.

### Discovery

Module-wired items implement `Discoverable`; modules list them flat in
`#[module(providers = [...])]`. Single-concern decorators
(`#[injectable]`, `#[mcp]`, gateway struct) emit `impl Discoverable`
directly. **Inventory-based** — the module list *is* the decorated
things; never enumerate controllers/providers by hand.

**Orchestrator pattern for per-method aggregation:** `#[routes]` scans
verbs, `#[resolver]` scans `#[query]`/`#[mutation]`/`#[field_resolver]`,
`#[scheduled]` scans `#[every]`/`#[cron]`/`#[after]`, `#[processor]`
scans `#[process(queue, ...)]`, `#[listeners]` scans `#[on_event]`,
`#[hooks]` scans phase attrs. The host struct owns the single
`Discoverable`; each method submits its unit to link-time `inventory`.
Use this for any concern where one provider owns several units sharing
the same `#[inject]` deps. Otherwise stay struct-level.

**Discovery is module-gated.** Every transport integrates only items
whose provider is *reachable* from the running app's root — a
`ReachableProviders` set from the access graph; each transport filters
its `inventory` against it. Linked but unreachable ⇒ inert, with a boot
`tracing::warn` so leftover code doesn't vanish silently. This is what
makes per-app subsets work.

**The gate is always the entry's owner** — what differs is who the owner
*is*, and that follows from what the entry is:

- **An entry that names a DI provider** — a method or role on something
  `#[inject]`ed by type (`#[process]`, `#[on_event]`, `#[query]`,
  `#[every]`, `HealthIndicator`) — is owned by **that provider**, so the
  gate is `ReachableProviders`.
- **An entry that names no provider** — a self-contained plugin the
  registry builds from its own config, like `SocialProviderEntry` — is
  owned by **the module providing the registry** (`SocialModule`), whose
  presence in the import graph is the gate.

Same rule either way, and it is why the second kind needs **no module of
its own**: only something injected by type does.

**Being buildable is not discovery.** Once an entry is discovered, its
own config decides its fate on the dual-path `#[config]` rule: complete ⇒
active, absent ⇒ inert + boot `warn`, partial/invalid ⇒ boot fails naming
it. Never conflate the two — a capability that cannot be constructed is
not "undiscovered".

**Structural gating where discovery is metadata.** `ReachableProviders`
exists because `inventory` is *link*-time: everything compiled is in the
registry, imported or not. Metadata attached from `Discoverable::register`
has no such gap — `register` only ever runs for a provider an imported
module owns — so a metadata-discovered surface (`HttpEndpointMeta`,
`McpHostMeta`) is gated by construction, with nothing to filter and no
inert-entry `warn` to emit. Pick the mechanism, then take its gate; never
bolt a `ReachableProviders` filter onto metadata to look symmetric.

### A transport aggregates; owning a mount is the exception

**A transport aggregates contributions from several providers onto one
mount point.** Controllers mount routes flat into one `Route`; resolvers
merge into one schema; `#[process]` collects per queue name;
`#[scheduled]`, `#[on_event]` and `#[mcp]` the same. **Owning a whole
mount is the exception and has to be justified** — because the moment one
provider owns a mount, a product with two features on that mount has to
fold them into a god-adapter, which inverts the layout
`features.md` mandates. That inversion is always the framework's defect
to fix, never the product's licence to flatten.

Two shapes implement it, and the choice follows from where discovery
lives:

- **Merge a link-time registry** (`inventory`), filtered by
  `ReachableProviders` — GraphQL, queue, schedule, events.
- **Merge container metadata** — MCP. Each `#[mcp]` host attaches an
  `McpHostMeta`; the *first* host on a path also attaches the one
  `HttpEndpointMeta` that mounts them all, so the transport's
  "a mount path is its owner's exclusive namespace" rule stays intact and
  a real collision (an `#[mcp]` beside a `#[gateway]` on one path) still
  fails boot naming both.

Merging introduces exactly one new failure mode, and it must be a **boot**
error naming both owners: two contributions claiming the same addressable
name. For MCP that is a duplicate tool name within a path
(`nest-rs-mcp/src/registry.rs`), because the protocol addresses a tool by
bare name inside an endpoint and the loser would silently be unreachable.

**And it raises exactly one new question: who *is* the mount?** A contribution
answers for itself; the mount's own identity — what a client is told it is
talking to — belongs to the **app**, never to whichever contribution
registered first, or the answer becomes a function of `imports = [..]` order.
So an aggregating surface whose protocol exposes a mount-level identity gives
the app a seam to declare it once (`McpModule::for_root(McpOptions { server, .. })`,
provider-less metadata read back at mount) and lets **at most one** contribution
refine it for its own mount, and:

- **the declaration replaces only what it states** — identity is declared,
  capabilities stay *observed* from the contributions, so an app can never
  advertise a surface nobody implements;
- **a declaration that reaches nothing fails boot**, and two contributions
  declaring one mount fail boot naming both;
- **undeclared is reported, not guessed silently** — a mount left at the
  *SDK's* own default identity is a boot `warn` carrying the remedy. Compare
  against the SDK's own constructor, never a literal, so the check cannot drift
  from the version the framework builds against.

This is the ecosystem's shape, not an invention: one server object created with
its identity, contributions registered onto it (TypeScript SDK), and a parent
that "retains its own name and serves as the orchestrator" when it mounts
children (FastMCP).

**WS is the one justified exception, audited and recorded.**
`#[gateway(path)]` owns its mount: two gateways on one path is the
transport's duplicate-self-mount boot error, and there is no seam to merge
their `#[subscribe_message]` arms. It stays that way because nothing
pushes a product to share a WS path the way MCP's clients push it to share
a URL — a socket per feature costs a client one more connection and
nothing structural, and the thing features actually need from each other
across sockets (fan-out to connections another feature owns) is already
solved by `WsServer<N>` namespaces *without* sharing a mount. So the
adapter shape holds: one `<feature>/ws/gateway.rs` per feature, each on its
own path. If a product ever genuinely needs two features' events on one
socket, that is a framework change on this same pattern (route by event
name, fail boot on a duplicate event) — reported, never worked around.

### Lifecycle hooks

`#[hooks]` submits phase-tagged methods (`#[on_module_init]`,
`#[on_application_bootstrap]`, `#[on_module_destroy]`, …) to `inventory`;
`App::run` drains per phase. Per-provider, run in `(provider, method)`
name order; init failure aborts boot, shutdown is best-effort.

## Surface crates — decisions, not mechanics

- **`nest-rs-http`** — the only activation seam is
  `HttpModule::for_root(...)` in imports; no public `.transport(...)`.
  Every `HttpConfig` field settable via `NESTRS_HTTP__*` env **and** the
  pinned struct — the framework-wide **dual-path config rule**, which
  applies to every `nest-rs-*` module.
- **`nest-rs-pipes`** — transport-agnostic, **one Pipe per file**,
  stateless (`transform(In) -> Result<Out, _>`, never a DI provider).
  Binds **per argument on all four transports**, two forms by design
  (orphan rule): HTTP wraps an extractor (`nest_rs_http::Piped<P, E>` /
  `Valid<E>`); GraphQL, WS and queue wrap the wire value
  (`nest_rs_pipes::Piped<P, T>` / `Valid<T>`, stripped by
  `#[resolver]`/`#[messages]`/`#[processor]`). A rejection surfaces as
  the transport's native error (400 / GraphQL error / WS error frame /
  job error). Global pipes exist on HTTP only. **Reusable pipes are
  framework primitives — never define one in an app.**
- **`nest-rs-schedule`** — `#[scheduled]` orchestrator; methods tagged
  with exactly one of `#[every]` / `#[cron]` (optional `tz`) /
  `#[after]`. Literals validated at compile time; presets/timezones at
  boot. `Scheduler` is a `Transport` via `TransportContribution`.
- **`nest-rs-queue` + `nest-rs-redis`** — backend-agnostic queue contract
  (`Job`/`Processor`/`ProcessMethod` + `#[processor]` + inventory seam)
  with Redis first-class (on `apalis`). Crate names follow the
  **storage** (Redis), not the framework (apalis). Queues identified by
  name (stringly-typed, known cost). Producer/consumer decoupled.
  Connection seeded via `QueueModule::for_root`; consumer activates via
  `QueueWorkerModule` (producer-only apps skip it). **No apalis types
  leak.**
- **`nest-rs-ws`** — **not a `Transport`**: the WS upgrade is an HTTP
  GET, so `#[gateway(path = "/ws")]` self-mounts on `HttpTransport`
  (inheriting port/CORS/TLS). `#[messages]` orchestrates
  `#[subscribe_message]` + `#[on_connect]`/`#[on_disconnect]`; one
  envelope `{event, data}`. Per-gateway namespace via `WsServer<N>`.
  A gateway **owns** its mount — the audited exception to *a transport
  aggregates*, recorded above; sharing state across gateways is what
  `WsServer<N>` is for, not sharing a path.
- **`nest-rs-mcp`** — also not a `Transport`, also an HTTP self-mount, but
  it **aggregates**: several `#[mcp]` hosts merge into one `CompositeHandler`
  behind one endpoint, because MCP namespaces tools per endpoint and clients
  point at a single URL. One host on a path is served verbatim; the merge only
  engages beyond that. A host contributes an `McpHost` (the object-safe
  `ServerHandler` view) — it never has to know it is sharing. Guard,
  `dyn McpToolContext` and `McpConfig` are container bindings, so they resolve
  **once per path**, not per host.

  **A host's `path` is a join key, not a namespace.** Unlike a
  `#[controller]`'s, nothing nests under it: it names the one endpoint the host
  joins, which is why peers writing the same path share it. So it is written
  whole — the URL a client config carries — and `DEFAULT_PATH` (`/mcp`) is what
  a bare `#[mcp]` takes. It is a **constant, not config**: a path a *decorator*
  declares is code everywhere here (`#[controller]`, `#[gateway]`), and
  `HttpConfig.global_prefix` already moves the whole surface. (A module that
  owns its whole mount *may* configure it — `NESTRS_GRAPHQL__PATH` does — which
  is why `HttpEndpointMeta::new` normalizes every path it is handed rather than
  trusting the caller.) A prefix was tried and removed — a prefix prefixes a
  namespace, and there is none here.

  **A host writes one decorated `impl`.** `#[mcp]` on the struct declares the
  host; `#[mcp]` on its inherent impl declares the `#[tool]` / `#[prompt]`
  operations, absorbing rmcp's routers, handler attributes and `get_info`, and
  deriving the advertised capabilities from the roles present. Descriptions come
  from the doc comment — the prose was being written twice. A host serving a
  hand-written `ServerHandler` surface (resources, completion) stays on rmcp's
  raw shape; see *Macros* above.

  **A failing operation talks to a language model.** `Opaque::opaque` is the
  framework's seam for that: the real error is logged at `error` on
  `nest_rs::mcp`, the model gets a constant message. Never hand a `Display`
  straight to a tool's caller — a `DbErr` carries schema, columns and sometimes
  values. A deliberate `McpError::invalid_params` is the opposite case and is
  returned directly.

  **Identity has two owners, and neither can shadow the other.** One endpoint
  reports one `serverInfo` and one `instructions` however many features share
  it, so: the **app** declares itself once (`McpOptions { server }`) — `name`,
  `version`, branding *and* `instructions`, because a feature library knows
  neither the binary's version nor, on a shared endpoint, the whole surface —
  and a **host** declares only which endpoint stands apart
  (`#[mcp(name = …, title = …)]`, optional, overriding the app's per field).
  `instructions` is deliberately not a `#[mcp]` argument; a host writing one is
  a compile error, and per-tool prose belongs to `#[tool(description = …)]`.
  Two hosts declaring one path fails boot naming both. Identity is **not**
  config — it has no `NESTRS_MCP__*` twin, which is why it travels in
  `McpOptions` beside the config rather than through a second call.
- **`nest-rs-openapi`** — import `OpenApiModule`; self-mounts
  `GET /api-json` + offline Swagger UI at `GET /api`. Document
  **composed** from the route table. Schemas via **schemars**;
  `#[api(...)]` enriches an op.
- **`nest-rs-social`** — open provider contract. **Flow-owning**
  `SocialProvider` trait: `authorize`/`exchange` default to the shared
  PKCE/CSRF flow (through `nest-rs-authn`'s `OAuth2Client`, whose
  `exchange` yields a `TokenSet`), so a standard provider implements
  only `profile`; a non-standard one (Apple's ES256 secret, id_token
  identity) overrides a step **without changing the trait**. A social
  provider is **not a DI provider** — never `#[inject]`ed by type, only
  reached through `SocialRegistry` as `Arc<dyn SocialProvider>` — so it
  has **no per-provider module**: `SocialModule` owns every entry and is
  the single import. Within that gate, credentials decide: complete ⇒
  active, absent ⇒ inert + `warn`, partial/invalid ⇒ boot fails naming
  the provider. A duplicate key, or a registry key disagreeing with the
  provider's own `key()`, **fails boot**. **`SocialModule` takes no
  configuration** — the entry names the provider's own `#[config]`, so
  discovering a provider is what loads its credentials, and the module, which
  never learns the provider exists, has nothing to declare. See the converse
  corollary under *`for_root` — one seam, one value, no chain*.
  A third-party provider crate is therefore exactly two
  files: `config.rs` (`#[config]` + `SocialProviderConfig`) and
  `provider.rs` (`SocialProvider` + `inventory::submit!` whose `build` is
  one `resolve_provider` call). Ships first-party GitHub + Google;
  third-party provider crates are **encouraged** through the same public
  seam. Keyed injection (`#[inject(key)]`) stays the tool for **static,
  compile-time roles** (primary/replica pools).

  **This extension-crate posture — a public behavioral contract +
  inventory discovery — is the template for any future open-ended
  library in the repo.**

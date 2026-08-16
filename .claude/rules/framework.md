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

**A refusal lands at the earliest site that can see the fact.** When a
declaration is wrong in a way something can *know*, the question is only
who knows it first — and that site owes the error, because every site
after it costs the developer a run, a boot, or a silence. The
provider-hosted decorators are the worked example, one fact
(`Container::get::<Host>()` answers only for a singleton under its own
type) refused at four different sites:

| Knowable at | Shape | Answer |
|---|---|---|
| the host's own decorator (scope is right there) | `scope = request`, `scope = transient` | compile error, reading `ProviderResidency::SINGLETON` |
| the impl half's expansion | an edge host — metadata only, no instance | the same compile error |
| the boot, from this app's composition | held under another key — `dyn Trait`, a `for_root`, a hand-written `Module` | `warn` + `INERT_HOST_HINT` |
| the boot, from this app's imports | module not imported | the same `warn` |

**Stated, never merely absent.** A refusal that reads a *missing* marker is
fillable: `ProviderResidency` was a bare `Singleton` trait for one audit round,
and `impl Singleton for PerResolution {}` — the line its own note recommended to
hand-written providers — put a `scope = transient` host back through the bound
silently. Every decorator that builds a provider now writes the fact, `true` or
`false`, so contradicting it is `E0119` and the hatch survives only where nothing
has spoken. Testable form: a trybuild snapshot per refused shape *plus* one that
tries the escape.

**A `warn` may name causes; it may not prescribe an edit the framework cannot
verify.** The same hint offered "list it in `providers` under its own type as
well" — and `providers = [Foo, Foo as dyn Trait]` runs the constructor twice,
so the decorators fire on one instance while every `Arc<dyn Trait>` consumer
holds another, with nothing to notice; on a hand-written `impl Module` the same
edit fails the boot. Five causes reach that one skip line and the container
cannot tell which, so it names them and stops.

**Escalate no further than the fact supports.** The last two rows are correct
in another composition, so they warn — a boot error there would refuse working
code. And **a `warn` whose sentence is wrong is worse than none**: that line
claimed *unreachable from app's module tree* about a provider written in
`providers`, sending the reader to check the one thing already true. One shared
sentence, every site, or the wording drifts per crate — `INERT_HOST_HINT` is
that shape, and `is_framework_owned` is the same shape for the *level*: it lived
at one site of five, so two demo apps warned every boot about an indicator the
framework owns.

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
real entity, and an entity cannot live in a zero-dep crate —
`DeriveEntityModel` roots its expansion at the call site's `sea_orm` and
offers no `crate = ` override, which *is* the entity-site exception. Their
contract is proved by `crates/nest-rs-cli/tests/e2e/scaffold.rs` — which
scaffolds a workspace, generates a resource, repoints `nest-rs` at the
working tree through `[patch.crates-io]`, and runs a real `cargo check`.

**The two are excluded for different reasons, and conflating them cost a
shipped defect.** `#[expose]` sits on an entity, whose own source
legitimately writes `sea_orm`. `#[crud]` sits on a **controller**, whose
source writes nothing but `std`, `nest_rs` and `crate::` — it has no excuse,
and it emitted `::uuid::Uuid` for three routes and one resolver argument.
`g resource` bootstraps `g auth`, whose claims type names `uuid`, so the
scaffold e2e had the dependency whether the macro needed it or not, and
passed throughout. **A generated tree witnesses only what it does not also
supply by accident**:
`crud_needs_no_dependency_the_controller_does_not_name` drops the auth
modules from the module tree, the guards from the controller and `uuid` from
the manifest, so what remains rests on the decorator alone.

**The path-rooting rule is now executed, not merely stated.**
`nest-rs-macro-hygiene/tests/integration/emissions.rs` reads every
`*-macros` source and fails on a path rooted outside the framework — an
allowlist of the framework's roots, not a list of banned crates, so a
decorator reaching for something nobody thought to ban fails the day it is
written. The scan is exhaustive over decorators and blind to feature
resolution; the compile witness is the reverse. Keep both, or the next
defect hides in the gap between them.

**That distinction is load-bearing.** The `integration` suite asserts on
the *text* the generator wrote, which catches a wrong dependency and can
never catch a template that emits code the compiler rejects — only a user
would find that. A template change is not done until the `e2e` suite has
run; it shares the repo's target directory, so a warm run is seconds.

### One decorator, one item shape

**An edge is a pair of decorators, never one name worn twice.** The hard "no"
in `CLAUDE.md` carries the reasoning; here is the table it binds, and it is
closed:

| Edge | on the struct | on the impl |
|---|---|---|
| HTTP | `#[controller(path)]` | `#[routes]` (or `#[crud]`, which re-emits under it) |
| WS | `#[gateway(path)]` | `#[messages]` |
| GraphQL | `#[resolver]` | `#[operations]` (or `#[crud]`) |
| MCP | `#[mcp]` | `#[tools]` |
| queue / schedule / events | `#[injectable]` — no mount, no provider-scope layers | `#[processor]` / `#[scheduled]` / `#[listeners]` |

**The struct half is named for the host role; the impl half for what it
collects.** `#[messages]` carrying `#[on_connect]` beside the message arms is
the precedent for the dominant-unit reading, and it is why `#[tools]` is right
for a block that also holds `#[prompt]` methods: rmcp routes both through the
one `ServerHandler` the expansion writes, so they are one host's operations.

**MCP is the one edge whose *struct* decorator is not its role word.** The role
word went to the impl half, where a host's methods are; `#[mcp]` keeps the
protocol's name, and cannot be misread as the `#[tool]` this crate re-exports
and which the same file carries. The role word is in the file (`tool.rs`) and
the module (`<Feature>McpModule`) too. Accepted asymmetry, not an oversight —
and **not a licence to make either name cover both shapes again.**

`#[tools]` is what absorbs rmcp's three-block shape — `#[tool_router]`,
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
  an empty tool list — the duplicate-tool boot check would go blind. rmcp
  answers that itself (`#[tool_router(vis = "pub(crate)")]`), and
  `DefaultToolRouter` / `DefaultOperationLayers` are the empty fallbacks the
  struct half falls through to for a host that has no `#[tools]` block.
- **Capabilities are derived, never restated.** A `#[tool]` method advertises
  `tools`, a `#[prompt]` method `prompts`. A host can no longer route
  operations it forgot to declare — the defect the CLI template itself shipped.

**The escape hatch is a host that owns its `ServerHandler`.** Resources,
completion and the rest are hand-written trait methods, and the sugar cannot
generate a second `impl ServerHandler`; such a host writes rmcp directly and has
no `#[tools]` block at all — `#[tools]` on a trait impl is a compile error
saying so. `demo`'s `posts` is that host, deliberately kept as the witness of
the raw shape.

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
verbs, `#[operations]` scans `#[query]`/`#[mutation]`/`#[subscription]`/`#[entity]`/`#[field_resolver]`,
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

### A new edge owes the same list — and the list is here

The edge vocabulary is closed (`architecture.md`); the **form** is open, and this
is what the form costs. Every line below is something all four request-carrying
edges do today, with the grep or the test that proves it. Adding an edge means
doing all of it or not shipping the edge — a transport that implements eight of
these is not "a smaller transport", it is a hole a developer discovers at the
worst moment, because the thing it left out is the thing they assumed.

**Read the numbered list as the checklist and the parenthesis as the proof.** A
line whose proof you cannot run is a line you have not done.

1. **Two decorators, one item shape each** — the host on the struct, a sibling
   named for what it collects on the impl. Both halves parse through one
   `DecoratorPair` const (`rg 'DecoratorPair' crates/*-macros/src/` names every
   pair), so the wrong shape is a compile error **naming the sibling**, and each
   pair ships a trybuild snapshot **per** wrong shape.
2. **Mandatory posture per operation** — `#[authorize(Action, Entity)]` or
   `#[public]`, with a trybuild snapshot for the no-posture case. Silence is not
   a posture: the refusal is what keeps *no authn/authz decision outside a guard*
   true, and it is the one item on this list that is load-bearing on its own.

   **The grammar is shared where it is the same grammar, and only there.**
   `PostureRules` in `nest-rs-codegen` words the declaration, the
   mandatory-posture refusal and the `bind = Service` rejection once; `#[tools]`
   and `#[messages]` take it verbatim. The other two parse their own, each for a
   stated reason, and the reasons are the difference — not drift:

   - **`#[operations]` (GraphQL)** accepts `#[authorize(Update, bind = Service)]`
     and `id_arg = ident`, which synthesise an id argument and an
     `Authorized<A, E>` proof. No other edge can express that, and carrying the
     option in the shared rules for two transports that reject it would be the
     abstraction paying for a case it does not have. Argued at the top of
     `nest-rs-codegen/src/posture.rs`.
   - **`#[routes]` (HTTP)** has an **optional** posture — a route's gate may also
     be `#[use_guards]`, which is why `request-layers.md` says the posture is
     mandatory *on the last three* — and two refusals that exist nowhere else:
     `#[authorize]` on an `#[sse]` route, and binding the posture to a handler
     *parameter* (`authorize_param`). A shared `take` returning `Posture` rather
     than `Option<Posture>` cannot serve it.

   So the testable form is per site, not one grep: `PostureRules` is the only
   wording of the two-transport grammar, and each of the four edges ships the
   no-posture (or, for HTTP, the contradiction) trybuild snapshot. **Two of four
   is the correct count here**, and it is written down so the next reader does
   not read a hole where an argument is.
3. **A class gate the posture emits** — `nest_rs_authz::<edge>::authorize`, whose
   *decision* is the shared `gate` so `#[authorize]` cannot come to mean five
   things. Missing ambient ability fails **closed**.

   **HTTP's gate is the one that does not call it, and must not.** The shared
   `gate`'s first rung is `is_visitor()` ⇒ `Unauthenticated`, which refuses every
   anonymous caller before looking at a grant. The sanctioned public-reads
   pattern — `#[public]` beside a hand-written `Authorize<A, E>` — needs a
   `define_visitor` grant to *satisfy* the gate, so `Authorize` open-codes
   `can_class` then `missing_scopes` and has no `Unauthenticated` verdict.
   Argued on `Ability::is_visitor`, which names the split: on HTTP the route's
   own posture asks whether there is a principal, on the in-band edges the gate
   does.
4. **Response masking the same posture arms** — never hand-written at the use
   site, and `unmasked` is the opt-out for a shape the value-level round-trip
   cannot see through. Which of two shapes depends on what the edge does with the
   value: `masked_value_for` when it must reconstruct the return type (GraphQL's
   non-nullable schema, MCP's `structuredContent`), so a stripped required key
   refuses the operation; `masked_reply_for` when it ships JSON (WS), so the key
   is simply absent. **Pick by the protocol, not by symmetry** — and either way,
   fail closed on a missing ambient ability. The witness is a test in
   `nest-rs-authz/tests/integration/<edge>/mask.rs` asserting a field grant strips
   a column **with no masking call in the handler body**.

   **HTTP arms neither function**, and that is a fourth mechanism rather than a
   gap: `#[routes]` installs a `RouteResponseShaper` chosen **by the parameter's
   type** (`ShaperProbe`, so an alias or a re-export arms identically), and the
   shaper omits a masked key from the body. Same fail-closed reading, nothing
   for the expansion to call — which is why that edge's witness proves the
   effect and cannot spell an entry point.
5. **Guards at two scopes** — `#[use_guards]` on the host and per operation, plus
   `#[force_guards]`, composed once per site and deduped by `TypeId`. A denial
   renders through one `denial_to_<edge>_error`, so a guard's refusal and a
   gate's refusal reach the client identically.
6. **A `Guard::check_<edge>` entry** on the trait, feature-gated like its
   siblings — **plus a marker trait, and the bound the decorators emit for it.**
   Every `check_*` defaults to `Ok(())`, so without the bound a guard bound where
   it has no entry passes everything silently. The pattern is `<Edge>Guard: Guard`
   in `nest-rs-guards` with a `#[diagnostic::on_unimplemented]` note, declared by a
   guard beside the `check_*` it attests, and asserted per declared guard through
   `nest_rs_codegen::guard_capability_bounds`. **All four edges assert, HTTP
   included.** The bound never proves a *method* exists — the `Ok(())` default
   guarantees that at every edge — it proves the author **declared** this guard
   checks this edge; an empty `impl Guard for X {}` satisfies the compiler and
   passes everything, and the marker is what turns that into an error at the
   binding site. `HttpGuard` is the one marker carrying no `cfg`: its three
   siblings gate a `check_*` that exists only when that edge is compiled in, and
   HTTP is the substrate the other three mount on, so no build of the crate lacks
   `check_http`. Witness: a trybuild snapshot per edge, binding a guard that does
   not check it at that edge's site. HTTP has **three** emitters —
   `#[controller]`, `#[routes]` and the `#[gateway]` struct, whose guards run on
   the upgrade — and each underlines the decorator the guard was written under,
   with a snapshot of its own (`unattested_guard_on_a_gateway` is the third).
7. **Per-argument pipes** — `Piped<P, T>` / `Valid<T>` stripped by the impl-half
   decorator, rejection rendered as the edge's native error, and the pipe runs
   **after** the gate so a refused caller never pays for validation and a
   validation message never doubles as an existence oracle.
8. **A named compile error for every layer family the edge does not bridge** —
   `reject_http_only_layers`. A silently ignored `#[use_interceptors]` is the
   defect that function exists to prevent; extend it rather than adding a second.
9. **Request scope + a data context** — `Scoped<T>`, and an executor+ability
   re-install per dispatch through `dispatch::with_data_context` so commit and
   rollback semantics cannot drift from the other edges'.
10. **`#[config]` + `for_root`** — one seam, one value, dual-path env.
11. **Error opacity** — an `Opaque` trait beside the edge's error type, whose
    `opaque()` logs the real error at `error` on `nest_rs::<edge>` and substitutes
    `nest_rs_core::OPAQUE_CLIENT_MESSAGE`. **The trait is per edge and only the
    constant is shared**, and that is a finding rather than a preference: the
    trait's output *is* the edge's error type, which is what lets `.opaque()?`
    infer from the enclosing function's return type. One trait generic over the
    output has three applicable impls, the receiver stops deciding, and every call
    site needs a turbofish.
12. **Discovery and its gate** — `Discoverable`, `ReachableProviders` for a
    link-time registry or structural gating for container metadata, and an
    inert-entry `warn` either way.
13. **Aggregation** — several providers at one mount, with a **boot** error naming
    both owners on a duplicate addressable name. Owning a whole mount is the
    exception and has to be argued (WS is the one audited case, above).
14. **A mount** — a `Transport` via `TransportContribution`, or an HTTP self-mount
    declaring its `EdgePosture`.
15. **`nest_rs::<edge>` span target**, level per layer, ≥1 structured field per
    event.
16. **Four witnesses** — an `integration` suite covering guards / pipes / scope /
    posture; a driver in `nest-rs-testing` if the protocol needs one; an adapter
    in `demo/` (`<feature>/<edge>/`); and a use site in `nest-rs-macro-hygiene`
    proving the decorators need no second manifest line.

Then the packaging: *Shipping a new capability* in `CLAUDE.md` (umbrella feature,
`pub use`, README + docs `## Install`, derive routing, its two witnesses). That
list is about **reaching** the capability; this one is about the capability being
the same shape as its peers once reached.

**Two known asymmetries, both deliberate and both recorded above** rather than
left for a reader to rediscover: a WS gateway owns its mount (audited exception
to *a transport aggregates*), and on GraphQL and MCP the operation *guard*
installs the ambient ability while on WS the *data context* does — because a
gateway is `Guarded`, so its upgrade already ran the real chain and there is
nothing to re-run in band.

**Two residual gaps, and the declaration sites now hold only the smaller one.**
A guard may declare a capability marker without overriding the matching
`check_*` — a deliberate line a reader can see, written next to the method it
should have been. Its larger twin is closed at every *declaration* site: an empty
`impl Guard for X {}` bound by `#[use_guards]` no longer compiles at any of the
four edges, `HttpGuard` being the fourth marker.

**The second gap is the global site.** `use_guards_global([guard::<X>()])` takes
no capability bound (`nest-rs-guards/src/builder.rs`), so an empty guard
registered there still passes everything, silently. Bounding it is not the
answer: a global guard legitimately serves whichever edges it implements, and
requiring `HttpGuard` would refuse a GraphQL-only one — the fix would be a
per-edge global list, four declarations where the developer wrote one.

**"Serves whichever edges it implements" is now true of all four, and was not.**
The pool reaches an operation at the site where the operation exists: HTTP bakes
it into the `RouteShaper`, WS folds it per message, and the two `Exempt`
transports fold it into their per-operation chain — one `compose` in
`dispatch/chain.rs`, no per-transport scope switch. What an `Exempt` endpoint
guard runs is `check_http`, against the request; what the site runs is
`check_graphql` / `check_mcp`, against the operation. **Two questions, so
neither answers the other**, and an edge that ran the first never shortens the
second.

MCP used to say otherwise, and it was a fail-open: `mcp_chain.rs` excluded the
pool from the site, so a global guard overriding only `check_mcp` was never
consulted — while its presence made the pool non-empty and disarmed the deny-all
`is_empty()` tail in `mcp_operation_guard.rs`, so registering it *opened* an
endpoint that refused everything without it. The seam that expressed the false
claim went with it: `McpOperationGuard::already_ran` reported HTTP-scope
execution and was subtracted from an MCP-scope chain, which is a category error
that can only ever suppress a check.

**Two residues, both reported rather than closed, both owner questions.**

- **Discovery is gated where the protocol says it is, and that is not a
  residue.** `initialize` / `tools/list` / `prompts/list` are rmcp server methods
  the endpoint's `check_http` is the only gate on. That was recorded here as a
  hole; the specification says it is the design. MCP "provides authorization
  capabilities **at the transport level**", the server "acts as an OAuth 2.1
  resource server", and "authorization **MUST** be included in **every HTTP
  request** from client to server" — with 401 for an absent or invalid token and
  403 for insufficient scope, both HTTP statuses. Nowhere does the spec
  authorize a JSON-RPC method individually, and it never names `tools/list` as
  needing a check of its own. So the mandated gate is uniform over every HTTP
  request, discovery included, and it is the one this edge runs.

  **The GraphQL comparison does not transfer, and that is the correction.**
  `_service` / `_entities` are gated in band because GraphQL has no
  transport-level authorization to be uniform over — one POST carries an
  arbitrary document, so the field is the only addressable unit. MCP's unit *is*
  the HTTP request. Reading the two as the same shape is what turned a
  conformant edge into a recorded hole. A `check_mcp` chain over discovery would
  be a layer above the standard, not the standard: build it if a product asks,
  and do not carry it here as a debt.
- **The in-band chains are never phase-validated, and only they.** A
  `boot_validate_*` makes a misordered chain — an authorization-phase guard ahead
  of the authentication-phase one whose principal it reads — a named boot failure
  instead of a deployment that denies everything with nothing to say why. It
  fails **closed** (the ability guard finds no principal and installs nothing, so
  `Repo` denies), which is exactly what kept it quiet.

  HTTP has it at the `#[routes]` mount and over the global bucket. **WS has it at
  the upgrade and not per message** — one of its two sites. `#[messages]`
  attaches an `HttpBootCheck` calling `boot_validate_guards` over `#[gateway]`'s
  own emitted specs; that chain is an HTTP `GET`, so it runs `check_http`, which
  is the entry the check is written about. The check lives in the impl half
  while the upgrade's guards are declared on the struct half because a gateway
  freezes its chains **at mount**, with the container in hand.

  **Per message it would be wrong, and this was proved by shipping it.** A
  per-message chain runs `check_ws_message`, while `validate_guard_chain` reads
  `produced_principal` / `expected_principal` — which describe `check_http`, and
  `AuthnGuard` keeps the no-op `check_ws_message` default by design. Applied
  there the check was wrong in both directions at once: silently green on a
  chain where nothing attaches a principal at all, and a false boot failure on
  the split-scope shape `authn-authz.md` sanctions. The phase-*ordering* half
  would transfer; the principal half does not, and making it honest needs a
  per-message notion of what "produces" means. **Owner question**, recorded in
  `guards-baseline.txt` with the two defects that closed it for a day.

  `#[operations]` and `#[tools]` cannot answer there: they compose through
  `SiteChainCell` on the **first dispatch that reaches the site**, after every
  boot check has run, and nothing at boot enumerates the sites. Closing them is
  therefore not a call to add but a link-time registry of sites — the shape
  `GraphqlLoaderRegistration` already has — submitted by both macro crates and
  walked once at boot against `ReachableProviders`. **Owner question**, and the
  fix belongs to both edges at once.

Closing the first gap would mean four `check_*`-carrying traits with no
defaults, and then a guard serving three edges needs three container
registrations — every execution site holds `Arc<dyn Guard>`, and a trait object
cannot be narrowed back. The remedy would cost more than the defect.

**That same arithmetic is why `check_http` stays on `Guard`.** Moving it to an
extension trait is the shape the roadmap reserved a name for, and it buys
nothing: **`nest-rs-guards` itself depends on `nest-rs-http` unconditionally**,
with no `cfg` and no optional flag, so every build that links the guard core
links the HTTP stack whatever the consumer asked for, and a `cfg` on the trait
method saves no bytes. That one manifest line is the whole proof — do not
restate it as a list of dependent crates, which is both longer and false
(`nest-rs`'s bare `guards` feature and `nest-rs-authz`'s `mcp` feature both pull
guards without naming `nest-rs-http` themselves, and half such a list is
dev-dependencies, which no consumer build sees). `cargo tree -i poem` on a
headless feature set names the crates a worker actually pays for — they are
elsewhere, and each is its own report.

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

  **`#[sse]` is a verb, not an edge.** It sits beside `#[get]`/`#[post]` in
  `#[routes]`, collapses to `GET` before the route table is built (so
  `#[sse("/x")]` beside `#[get("/x")]` is the ordinary duplicate-route error),
  and owes none of *A new edge owes the same list* — it carries the edge's
  guards, pipes, posture and document verbatim. What it owns is the response:
  the handler returns an `SseStream`, the decorator writes the
  `text/event-stream` and arms the ceiling. **Four** refusals, each a named
  compile error, and they are named rather than counted because the count is
  what drifted: `#[authorize]` (masking has no wire model to reconcile against —
  a capability-only guard is the pattern), a **shaper parameter** — a
  hand-written `Authorize<A, E>` or a `Bind<A, S>`, which is the second and
  less obvious way to the same place, and on a stream the worse one, since the
  mask waves an opaque body through while the document says it masked — the
  response-decorator family in one sentence (`#[http_code]` / `#[redirect]` /
  `#[response_header]` all shape a response that *completes*), and
  `#[api(response_content_type)]`.

  **The stream ceiling lives in `HttpConfig`, and the namespace is the whole
  argument.** `NESTRS_HTTP__SSE_MAX_CONNECTION_SECS` is the third instance of
  one security control — a long-lived connection authenticates once and then
  replays those privileges — so it takes its peers' reading, default and `0` ⇒
  unlimited spelling verbatim. It does **not** take their namespace: an `sse`
  one would mean an `SseConfig`, which under *one seam per config* would owe a
  `for_root`, which would mean a module for a response shape. SSE is not a
  module, so the knob belongs to the transport that serves it. Asymmetry
  argued, not silent.
- **`nest-rs-pipes`** — transport-agnostic, **one Pipe per file**,
  stateless (`transform(In) -> Result<Out, _>`, never a DI provider).
  Binds **per argument on all five transports**, two forms by design
  (orphan rule): HTTP wraps an extractor (`nest_rs_http::Piped<P, E>` /
  `Valid<E>`); GraphQL, WS, MCP and queue wrap the wire value
  (`nest_rs_pipes::Piped<P, T>` / `Valid<T>`, stripped by
  `#[operations]`/`#[messages]`/`#[tools]`/`#[processor]` — on MCP the carrier
  goes *inside* `Parameters<…>`, which is what the protocol deserializes an
  operation's arguments into). A rejection surfaces as the transport's native
  error (400 / GraphQL error / WS error frame / `invalid_params` / job
  error). Global pipes exist on HTTP only. **Reusable pipes are
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

  **`#[input]` stays re-exported at the queue edge and stays off the queue
  scaffolds — both on purpose.** Unknown-key rejection is the right default
  where the sender is an untrusted caller; a job payload's sender is the
  producer, possibly one deploy ahead, so the same rejection dead-letters
  the job on attempt 1 instead of ignoring the field the worker does not
  know yet — retries never help, the payload never changes. The scaffold
  therefore writes tolerant serde derives; a payload that wants *value*
  validation may still opt into `#[input]`, accepting that its producer and
  workers now version together. Asymmetry argued, not silent.
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

  **A host writes one decorated `impl`, carrying the same request layers every
  other edge has.** `#[mcp]` on the struct declares the host and takes
  `#[use_guards(...)]`; `#[tools]` on its inherent impl declares the `#[tool]` /
  `#[prompt]` operations, absorbing rmcp's routers, handler attributes and
  `get_info`, and deriving the advertised capabilities from the roles present.
  Each operation takes `#[use_guards]`/`#[force_guards]`, a **mandatory**
  posture (`#[authorize(Action, Entity)]` / `#[public]`) and per-argument pipes.
  The expansion emits a delegating wrapper carrying `#[tool(name = …)]` rather
  than rewriting the authored body, so the developer's method keeps its real
  signature and the wire name stays the authored one. A description is
  **`#[tool(description = "…")]`** — the declared form, because the sentence a
  model reads is behaviour, not commentary, and a workspace that carries no
  comments must still be able to state it. A doc comment is the *fallback* for a
  codebase that does write them, so the prose is never authored twice; an
  operation with neither **does not compile**. A host serving a
  hand-written `ServerHandler` surface (resources, completion) stays on rmcp's
  raw shape; see *Macros* above.

  **A failing operation talks to a language model.** `Opaque::opaque` is the
  framework's seam for that: the real error is logged at `error` on
  `nest_rs::mcp`, the model gets `nest_rs_core::OPAQUE_CLIENT_MESSAGE`. Never hand
  a `Display` straight to a tool's caller — a `DbErr` carries schema, columns and
  sometimes values. A deliberate `McpError::invalid_params` is the opposite case
  and is returned directly.

  **The reasoning was never MCP's**, and the seam is no longer either: a GraphQL
  error frame and a WS error frame are read by clients just as untrusted, so
  `nest_rs_graphql::Opaque` and `nest_rs_ws::Opaque` are the same trait beside
  their own error type. Three traits rather than one generic over the output, for
  the inference reason under item 11 of *A new edge owes the same list*.

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

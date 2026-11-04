---
paths:
  - "crates/nest-rs-core/**"
  - "crates/nest-rs-config/**"
  - "crates/nest-rs-testing/**"
  - "crates/nest-rs-social/**"
  - "crates/*/src/module.rs"
---

# The container — composition, configuration, discovery, lifecycle

The DI container is internal by decision (`CLAUDE.md`, hard "no"); extend it.
The product-facing half — the three module shapes, the configuration ownership
table, a `#[config]`'s namespace — is `architecture.md`. This file is the
framework half.

## Composition

- **`App::builder().build().await` runs four phases whatever the call order**:
  seeds (runtime values from `main`), collect (modules queue async factories),
  factories (awaited; a seed wins over a factory of its type), register
  (providers built from seeds and factory outputs). `App::builder()` is the
  one boot: `main` holds only `App::builder().module::<AppModule>()`, and a
  suite boots the same module, so what the app declares — its transports,
  its app-wide layer pools — runs in both.
- **The app-wide layer pools are declared by the module the app boots**:
  `GlobalGuards`, `GlobalPipes`, `GlobalInterceptors` and
  `GlobalExceptionFilters`, each a provider of its root's `providers`, never
  a builder call `main` alone runs. A declaration listed by a module the app
  imports, or registered by no module's providers, fails the boot at the
  wiring step naming it — a feature never chooses what covers every route —
  as does a second declaration of one family, naming the module listing
  each; a layer the declaring module does not reach fails the access graph,
  and so does one that is not a singleton, as a site's layer does
  (`ScopedLayerError`): a pool resolves its layers once, at mount. The pools
  are checked before every module's wiring.
- **A factory comes in three kinds** — a default, a declaration, a dyn binding
  — each with or without the factory outputs it reads, written as one tuple
  (`FactoryInputs`, sealed): a new arity is an impl, never a new form.
- **A `register` refuses, never panics**: what it cannot build it files with
  `ContainerBuilder::refuse` and returns, as a factory returns `Err`. So does a
  site a transport composes at `configure` against the built container: a
  route returns its refusal through the route table, failing `configure`
  naming the route and its controller; a gateway's message table, mounted
  with no `Result`, files it, mounts what denies, and the boot fails on it
  once the transports are configured — `App::run` and the test harness read
  it (`.claude/decisions/site-refuses-through-its-container.md`). A factory
  is queued in `collect`, so a module or setup wiring another imports it in
  both phases, through `ContainerBuilder::import` — the one way into a
  module's phases, held by `Collecting` and `Registering`
  (`.claude/decisions/module-phases-by-import.md`); one queued later is
  refused (`LateFactoryError`), and a module imported in a collect alone
  (`UnregisteredModuleError`). A container built by hand
  (`Container::builder()`) is in the register phase from the start and drains
  no factory; `ContainerBuilder::build` returns the first refusal it holds, in
  the boot's order (`ContainerError`), never a container without what it
  refused.
- **Providers are singletons unless scoped.** `scope = request` is built per
  request from the singleton root: it may inject singletons and other
  request-scoped providers (one instance each per request), never the reverse,
  and a handler reaches it through a `Scoped<T>` parameter. `scope = transient`
  is rebuilt on every resolution. A singleton injects neither, optionally or
  not (`ScopeViolationError`). Providers waiting on each other fail the boot
  naming the chain, whatever their scope and whichever modules list them
  (`ProviderCycleError`); a cycle among
  factories registered by hand, which no module descriptor shows, panics at
  first resolution naming it. **A type has one provider whatever
  its scope**: a second registration fails the boot naming both and their
  scopes (`DuplicateProviderError`) — never warned and replaced.
- **Modules compose by type or by configured value** (`DynamicModule`), and a
  configured value is the module its required `DynamicModule::module` names, as
  NestJS's `DynamicModule { module }` is — declared, never read off what
  `register` wires. Registration is idempotent, so a diamond builds once;
  dynamic imports are not deduplicated — what they wire per app is, through
  the module they import by type (*`for_root`*).
- **The container is flat and the access graph is the contract.** Visibility is
  Rust's: hide an implementation module-private and bind a `pub trait` with
  `Foo as dyn Trait`; there is no `exports` list. The binding takes a singleton
  only (a `const` assertion on its residency) and holds that one `Foo` under
  its own type and each trait it is listed with, so a listing names a provider
  once. `#[module]` records each provider's injected types, and the boot fails
  (`AccessGraphError`) on a provider injecting what its module neither owns,
  imports transitively, nor receives as global infrastructure: what a seed or
  a factory makes is app-global, as NestJS's `TypeOrmModule.forRoot` and a
  global `ConfigModule` are, and a provider a module lists is import-gated —
  being global licenses no data access outside `Repo` (`data-layer.md`). What
  a hand-written `register` installs is neither: the graph cannot see it, so
  whatever registers after it reaches it with no import and one registered
  before it is refused (`UnbuiltProviderError`) — app-wide infrastructure is a
  factory's output, and a gated provider is listed in `providers`. It
  governs `#[inject]`, optional or not — an optional dependency no module the
  app reaches provides is absent, never a leak — the `#[use_*]` layer lists (a
  provider-hosted site's through the `HostLayers` entry its decorator
  submits, since the host's struct is `#[injectable]`'s, and an app-wide
  pool's through what its declaration attaches at register; a site resolves its
  layers once, at boot, where a request-scoped one is never built and a
  transient one would be shared by every run, so either fails the boot naming
  its scope, `ScopedLayerError`), what a decorator's generated code resolves
  on its host's behalf (the `HostRequires` entry, never checked for scope:
  `#[crud]`'s unit of work), and
  every provider a handler resolves through a parameter whose type implements
  `ResolvesProvider`, checked for reach but never for scope, since a request is
  its consumer; `Container::get` at runtime is an unchecked escape hatch. **A provider a module lists is built, or the boot
  refuses naming why** (`UnbuiltProviderError`, after the access graph): an
  import cycle is tolerated while no provider waits across it. One built
  without an optional dependency the boot registers afterwards would run as if
  it were absent, and is refused too (`LateOptionalDependencyError`).
- **A test override replaces its provider before anything is built from
  it**, as NestJS's `overrideProvider` does: installed before any module
  collects, it is claimed by whatever would register its key — a seed, a
  factory that then never runs, a provider of any scope that is then never
  built — and one nothing claims fails the boot (`UnclaimedOverrideError`).
  The double keeps its provider's module and scope, so the access graph holds
  its consumers to production's imports, and a second registration of its
  key stays a duplicate (`DuplicateProviderError`); it injects nothing, so
  neither the cycle walks nor the hook order read the replaced provider's
  dependencies. A binding `as dyn Trait` is overridden whole through either
  key, and a consumer of the key left absent fails the boot
  (`OverriddenDependencyError`).

## `for_root` — one seam, one value, no chain

**A module is imported bare to read its environment, and `for_root` only
pins.** Every module owning a `#[config]` implements `Module` by type, so its
bare import (`HttpModule`, `RedisModule::<Cache>` for a named instance) queues
the environment over `C::default()` and does its whole wiring; a module is
configured in exactly one place, by exactly one value: `Module::for_root(x)`,
where `x` carries everything the app pins about it. The `*Setup` it returns is
opaque — no public method, no second constructor on the module type. A
declaration that does not fit makes `x` grow a **field**, never the seam a
**method**. Held by review: no inherent `impl` on a `*Setup`, and no inherent
`pub fn` on a module type besides `for_root`. `ConfigModule` is the one
carve-out — its `provide_feature` (what a bare import queues),
`provide_root` (what a `for_root` pins), `setup` and `resolved`
(`resolved_for` per instance) are the primitives every other seam is built
from, and `provide_feature`, `provide_root` and `resolved`, the reads a
`register` or a factory makes of the config, are public for third-party
drivers. The first config a boot loads, through either, registers the active
`Environment` and files the undeclared-environment line
(`.claude/decisions/undeclared-environment-is-production.md`), so no app
imports anything for it; a provider injecting it in an app that loads no
config is refused at boot with the remedy `nest-rs-config` submits for it, in
the binary and its suites alike — the harness seeds none. A seam one framework crate needs from another lives
in its crate's `__private` (`macros.md`); one only its own crate needs stays
private.

`x` has three shapes and only three, and none is `None` — nothing pinned is
the bare import:

- **`C`**, the module's `#[config]` — the default: the environment over `c`,
  per field.
- **`impl Into<ForInstance<I, C>>`** — only for a module whose resource an app
  may open more than once: the default instance takes `x`, a named one
  `(Marker, x)`. A value names the instance at `for_root` because Rust applies
  no default type parameter while inferring an expression; a bare import,
  where it does, names it as a type argument (`RedisModule::<Cache>`).
- **`impl Into<MSetup>`** — only when the module carries a declaration with
  **no environment twin** (MCP's identity and access, static files' embedded
  folder, the `Scope`s a resource server's rules gate on — the variable
  advertising scopes replaces what is advertised, never the vocabulary): that
  declaration `D` alone, the config alone, or the declarations before the
  config as a tuple, `(D, C)`, as a marker precedes its config. The setup
  stays opaque: the conversions are its only constructors.

A setup that only pins its config is `ConfigSetup<M, C>` (`ConfigSetup<M, C,
I>` per instance) behind a named alias, built by `ConfigModule::setup`; a
hand-written setup is for one whose declarations go beyond the config.

**A setup imported twice wires once**: what it wires per app — a transport,
a mount, a close, a registry — runs in the module it imports by type from both
of its phases, never in its own, so a feature importing the module bare beside
the app's `for_root` wires nothing twice; what an import site chose is a
declaration, so two refuse naming both. An app runs one transport of each
type (`ContainerBuilder::provide_transport`). Held by `nest-rs-testing`'s
setups table, which imports every first-party module owning a config bare,
pinned, and both together.

- **A bare import is a dependency declaration and the environment-only form
  at once**: the module it names does its whole wiring by type.
- **Who gets a `for_root` is `architecture.md`'s** (*Configuration*): every
  `nest-rs-*` module owning a `#[config]`, never one owning none
  (`.claude/decisions/config-one-seam.md`). **A config a reached provider
  injects loads itself**: `#[config]` files the type
  (`InjectedFactory`), and once every module has collected the boot queues
  the factory of each one a reached module's provider requires that no
  import queued — module-gated by that provider; a framework module reading
  its config in `register` queues it from its imports through the config
  crate's private seam, and a seed of a filed config is held to its checks
  in a container built by hand too. Held by `nest-rs-testing`'s config
  suite.
- **A value an import site chose is a declaration**
  (`ContainerBuilder::provide_declared_factory`, carrying its remedy sentence),
  and so is the trait object a `provide_factory_dyn` binds.
  The boot outcomes `architecture.md` lists — supersede,
  `ContestedDeclarationError` naming both imports and their positions before
  any factory runs — each have a behaviour test in `nest-rs-config`. We
  exceed NestJS here on purpose: a silently dropped declaration is a swallowed
  error.
- **One exception: `OpenTelemetry::init()` / `init_with(config)`** belongs to
  `main`, because the global tracer must exist before any module registers and
  its guard's `Drop` flushes — never a `#[nest_rs::main]` key, which could not
  take the pinned config (`.claude/decisions/telemetry-reads-the-specification.md`).
  Any other claimed exception is reported, not written.

## Configuration — the loader loads, the consumer judges

`nest-rs-config` resolves each variable through the tiers — deployment, the
`.env` cascade, defaults — except that a config pinned in code switches its
whole namespace to deployment-over-pin, and the cascade is not read beside a
pin: a field set to its default cannot be told from one `..Default::default()`
filled, so the cut is the namespace's, and each cascade key it shadows is
reported (below). It hands values over, assigns them no meaning, and never
arbitrates between variables or tiers.

- **The cascade has two layers**: the root's files, and under a declared
  development or test profile the app crate's own directory (the one
  `#[nest_rs::main]` captures, `apps/<bin>/` in a workspace), read whole over
  the root's. What identifies one binary of a workspace — its port, its
  audience, the issuer's key — lives in that layer, never in a pin made to
  dodge the shared file; a deployed profile reads the root alone. The loader,
  the test harness and `nestrs doctor` read it alike, each held by its tests.

- **`<KEY>_FILE` is a spelling of `<KEY>`, never a second variable**, and every
  namespaced variable has it (the kernel's pre-container `<PREFIX>_LOG_FORMAT`,
  `<PREFIX>_LOG_SOURCE_LOCATION` and `<PREFIX>_ENV` do not), so a secret never
  has to sit in the process environment. Every file is read through the one
  bounded reader (`read_material`), at the boot: only the HTTP server's
  certificate renews in place, and a consumer that comes to re-read on
  renewal reuses it. The deployment chooses the spelling: either one present
  in the deployment tier shadows both in `.env`; both set in the answering
  tier is refused naming both — the loader's one refusal, about spelling,
  never meaning. A refusal never
  repeats a value read from a file.
- **A combination of variables is the consumer's**, refused at boot in the one
  constructor every path reaches — config-driven or built in code — naming what
  is set, never settled by picking one.
- **A `#[config]` type has one base, its `Default`, which is production-safe,
  and an undeclared environment is production**: a development convenience —
  plain HTTP, a public surface (the documentation, GraphQL's playground and
  introspection, Server-Timing), a committed artifact's emit, a seed — is a
  `Surface` field, `ByProfile` in `Default`, resolved where it is used: open
  only under a declared development or test profile (`Environment::is_deployed`
  is the one test; an emit waits for development alone, since parallel suites
  would rewrite the file), never by the cargo build profile, so a pin over
  `..Default::default()` closes nothing it does not name and opens nothing in
  any deployed or undeclared profile, where an opening is a written
  `Surface::Open` or a set variable; and a profile read in the overlay would
  rewrite a pin, so a written value is never touched. **An address or a
  credential is never a default**, in any profile: the deployment names it, or
  the profile's `.env` file — the scaffold's for its `compose.yml`, the demo's
  for the dev container. The kernel's console, built before any config, reads
  the same profile through the config crate it links (`ProfileSource`), and
  `LogFormat` takes the same shape. Held by the field's type and each member's
  unit test.
- **A seed pins the value, never the checks**: a seeded config passes every
  bound and `validate()` before anything reads it (`Config::check`, run where
  the seed makes its factory skip), on every boot that drains factories, and
  at `ContainerBuilder::build` for a container built by hand, which drains
  none (`ContainerError::SeedRefused`).
- **A `#[config]`'s overlay is derived from its struct**, each key its
  field's name, each reader its type's (`ConfigField`), so no field is
  reachable from code alone (`.claude/decisions/config-overlay-derived.md`);
  `from_env = "manual"` is for fields that depend on each other. **`None` is
  off and nothing else**: a duration is a `Duration`, or an `Option<Duration>`
  only where an off exists; a count the same, `0` from the environment its
  `None`; a default sits in `Default`, never behind a `None`, and no field
  names a unit. Held by the derive's compile errors and its reads, which read
  off from the field's type and refuse a count pinned at `Some(0)`.
- **A duration a deployment sets has a floor and a ceiling, and so does a count
  read through `CountBounds`**, refused naming the variable whether pinned or
  read, never clamped. The floor is where the thing stops working; the ceiling
  is where a value becomes a slip, **and never above what the library or kernel
  it reaches accepts** — no value the boot accepts may panic or be refused
  below it. A range names
  nothing but its ends and their reasons: the derive binds it to the field it
  bounds, whose name is its variable's key — a duration's unit after it — and
  whose type says whether `0` is off (an `Option<Duration>`; a pinned
  `Some(Duration::ZERO)` is refused, since off in code is `None`), so no site
  types the key. Held by the `BoundedSetting` the derive binds, whose rustdoc
  has the mechanics, including a hand-written overlay's own binding.
- **A variable no config claims is reported, never ignored** — at `warn`, once,
  by name and never by value: a key under a namespace this binary read that no
  config read, a near miss of such a namespace, and a cascade key in a
  namespace a pin in code cut off the cascade, naming the import that pinned
  it. Everything else is silent by design: the root's `.env` serves several
  binaries (what is one app's alone sits in its own layer), and a namespace
  this binary does not read is another binary's. The near-miss tests are narrow for that reason
  (the algorithm is `unclaimed.rs`'s `//!`); a diagnostic that fires on correct
  configuration teaches operators to filter its target out. *One namespace, one
  type* (`architecture.md`) is refused at the read as
  `ConfigError::SharedNamespace`, and a named instance whose namespace a linked
  config or another marker holds as `ConfigError::SharedInstance`.
- **`nestrs doctor` links no framework crate**, so its reading of a variable is a
  second implementation of the loader's, held to it by a differential test in
  `nest-rs-cli`'s suite; it does not run the unclaimed report.
- **A structured `#[config]` value is a payload**: decoded by
  `ConfigService::json`, never by a config's own `serde_json`, and redacted at
  the `ConfigError` sink (`CLAUDE.md`, no payload value in an error).
- **A TLS client's material is `ClientTls`**, read under its namespace's
  `TLS_CA_CERT`, `TLS_CERT` and `TLS_KEY` through a field named `tls`, the
  name every refusal gives them, so another name is refused at the read
  (`ConfigError::MisnamedField`) — or the variables a specification
  names for them (OTLP's `OTEL_EXPORTER_OTLP_CERTIFICATE`, `CLIENT_CERTIFICATE`
  and `CLIENT_KEY`), refused under those names: the one client vocabulary lives
  beside the loader, its checks run in the consumer's one constructor, and a
  driver converts it to its library's shape rather than writing a `*Tls` of
  its own. No switch turns verification off (`CLAUDE.md`), held by
  `clippy.toml`'s disallowed methods.

## Discovery and its gate

Module-wired items implement `Discoverable` and are listed flat in
`#[module(providers = [...])]`; the list *is* the decorated things, never a hand
enumeration of controllers. Where one provider owns several units sharing its
`#[inject]` dependencies, the impl half orchestrates: the host owns the single
`Discoverable` and each method submits its unit to link-time `inventory`.

**Discovery is module-gated** (`CLAUDE.md`, hard "no"). `inventory` is link-time,
so every transport filters its entries against `ReachableProviders`, the access
graph's set for the running root; linked but unreachable is inert. A reader
takes the set through `ReachableProviders::of`, and a container that carries
none — built by hand, not sealed by a boot — is refused naming the reader
(`UngatedDiscoveryError`): there is no ungated mode, a test included, which
seeds the set it means. The gate is
always the entry's owner: an entry naming a DI provider is gated by that
provider's reachability; an entry naming none (`SocialProviderEntry`) names a
module of its own, and only an app reaching that module (`Composition::reaches`)
holds it. Never gate such an entry by its registry's module alone: a crate no
path names is not linked, so a dependency line opens nothing, and the import
that names the crate is the one visible opening. **Metadata attached from
`Discoverable::register` is gated by construction**, since `register` runs only
for an imported module's provider; never bolt a `ReachableProviders` filter
onto it for symmetry. **Being buildable is not discovery**: a discovered
entry's own config decides its fate — complete is active, absent is inert with
a boot `warn`, partial or invalid fails the boot naming it. **A registry the
app's code reads is filled by `provide_wiring`, before any hook — never by a
lifecycle hook**: the kernel runs every wiring once, in registration order, on
the sealed container, and a hook that ran first would read the registry empty.
A wiring is synchronous, so no I/O slows the boot there; one that fails ends it
with `WiringFailedError` naming the registry.

**An inert host is reported at the level its cause earns**, and the boot reads
the cause (`InertHost`, from the module descriptors and the app's
`Composition`). The app's own code — a host in its roots' crates that no
imported module lists, one registered outside every imported module — is a
`warn` with `cause` and a `hint` naming that cause's remedy. A dependency's
host the app neither imports nor registers — the framework's, a package's, a
shared library crate's — is said at `debug` (`cause = "dependency"`). The cause
is read off the composition, never off a crate's name: a prefix is no evidence
of who wrote a crate. **A host the app reaches whose edge's transport no
module attaches is a cause too**, named at `warn` with its module and the
import serving it, by `App::run` once its transports are attached, never at
the seal, which `TestApp` and `HeadlessApp` serve with transports they build;
each edge's crate declares what its hosts run on (`EdgeRuntime`), and
`nest-rs-health` what its indicators run under (`HealthModule`'s probes),
held by a test per edge and one for health. A run with no transport at all says so at `warn` and stops for
`StopCause::NoTransport`.
Neither refuses: a shape a harness serves is no defect, and a refusal would be
a break (`.claude/decisions/adapter-imports-its-transport.md`). Three
disciplines bind every boot diagnostic:

- **Escalate no further than the fact supports** — a shape correct in another
  composition warns or says less; a boot error there refuses working code.
- **A hint prescribes only the edit its cause has, once it knows the cause**;
  where the cause cannot be read, `INERT_HOST_HINT` names every cause and
  prescribes none.
- **A `warn` whose sentence is wrong is worse than none**, so one shared
  sentence serves every site, and the level is one shared predicate
  (`InertHost::is_the_apps`).

## A swappable concern ships an extension contract

Anything a third party could plug an implementation into owes a written
contract: the trait, the seam that makes an implementation reachable, and the
sentence for more than one. **If it cannot be written, the concern is not
swappable — say so in the crate's `//!`**; a client with no trait and no
sentence is a closed door that looks open. A third-party transport is
supported (`ContainerBuilder::provide_transport`); it adds no edge. The unit is the **port**, never the
crate housing it (`nest-rs-authn` holds the `Strategy` port beside `JwtService`,
shared infrastructure). Ports & Adapters is `architecture.md`; two independent
questions shape a port:

- **Who owns the driver problem** — answered by the crate that *binds* the port.
  *Delegated*: a library is already multi-driver (`sea-orm`, `object_store`), so
  the port exists to swap the vendor, is thin, and declares no config — a thin
  port there is correct, not unfinished. *Owned*: nothing abstracts the concern,
  so nestrs defines the trait, seam and arbitration (`ThrottlerStore`,
  `OccurrenceLock`, `JobProducer`, `SocialProvider`, `Strategy`).
- **What selects the active implementation.** *By import*: one, chosen by the
  imported module; two is a boot error worded once per port as a
  `BACKEND_REMEDY` declared in the file holding what a backend supplies, never in
  a `module.rs`, and it may name the first-party binding as the remedy. None,
  where a provider injects the port, is the same sentence: the file submits it
  as the `DependencyRemedy` of the type a provider injects — the port's trait
  object, or its handle where the binding registers one (`Jobs`) — which the
  unmet dependency names in place of the kernel's, as the module opening a
  resource does for the resource's type; a contract no binding registers
  carries a remedy naming the handle. *By type
  parameter* (`AuthnGuard<S: Strategy>`): two instantiations are two types, so
  no arbitration is owed. *By configuration*: the import opens the gate and
  configuration decides which members are active, zero to all — or, for a
  delegated port, which of its library's backends: the URL's scheme
  (storage's `s3://`, `memory:///`).

**A port promises only what every backend it ships holds**; a looser backend
either does not declare the capability, or the port is worded loosely enough and
the backend's own bound is written on the backend. **A port owns a config
namespace if and only if its contract requires the integrator to honour a
config** (the throttler's does); otherwise the adapter's config takes the
namespace its path gives it (`architecture.md`) —
`.claude/decisions/port-config-namespace.md`. **Each port selected by import
has its contract written on one driver page, in one order**: the contract,
the binding and its `BACKEND_REMEDY`, the budget it declares under the port's
net, what a failure of its backend becomes, TLS, the proof at boot, health,
and the test that holds it — each excerpt lifted line for line from code the
framework compiles, its binding or the port's suite; the docs' *Extending
NestRS* section lists the pages.

**What runs around a unit is an ordered list or a port, never a single `dyn`
slot**: a concern wrapping every unit is a unit-stage interceptor its module
brings, a driver the data unit of work through the one `UnitOfWork` port. A
slot holds one implementation, so the second concern needing it has nowhere
to go (`.claude/decisions/one-execution-pipeline.md`).

**A resource an app may open more than once is generic over its instance**
(`.claude/decisions/named-instances.md`), one step per piece:

- the marker is `#[instance] pub struct Cache;`, named by its ident;
- the resource, its setup and each binding take `I: Instance = Unnamed`, so
  one instance is the app's spelling unchanged; a binding is a hand-written
  `impl<I: Instance> Module`, since `#[module]` is not generic;
- the bare import is the typed one (`RedisModule::<Cache>`), whose `collect`
  calls `ConfigModule::provide_feature::<I, C>`, and `for_root` takes `impl
  Into<ForInstance<I, C>>`, which `ConfigModule::provide_root` pins; both read
  `C` under the instance's namespace and hold it as `I::Of<C>`, which a
  factory reads back with `ConfigModule::resolved_for::<I, C>`;
- a helper called in expression position takes `I` from its caller's own type
  parameter, never from a default;
- every refusal and every line names its instance's variables.

**An open-ended library is a public behavioural contract plus inventory
discovery**, and `nest-rs-social` is the template. `SocialProvider` owns the
flow: `authorize` / `exchange` default to the shared PKCE/CSRF flow through the
provider's `OAuthClient`, so a standard provider implements `profile` and a
non-standard one overrides a step without changing the trait. A social provider
is not a DI provider — reached only through `SocialRegistry`, in no `providers`
list — yet it has a module, `<Vendor>SocialModule`, which an app imports to open
it; a third-party provider crate is `config.rs`, `module.rs` and `provider.rs`.
A duplicate key, or a registry key disagreeing with the provider's `key()`,
fails the boot. A static role a resource plays twice is an instance marker
(`Instance`, above), never a string key.

## Every wait the framework owns is bounded

A backend is trusted to answer, never to answer in time: a port call the
framework awaits has a **net** of the framework's, and past it the call is a
backend that cannot answer. **The net is not the backend's budget** — an adapter
still bounds each command it sends, so an outage arrives as the backend's own
error with its cause, and the net fires only on a backend that stopped bounding
itself. **The boot holds every budget under each net reaching it**
(`nest_rs_core::{Budget, Net}`), refused as soon as the resource exists: the
module opening a resource declares its budget, as does each binding handing it
to a port or installing it around every unit of work, and whoever arms a net
declares it — a binding over the resource it hands its port, a guard around its
strategy; a default is pinned by a unit test wherever both constants are
visible. An edge's deadline is the deployment's, not a net: only its default is
ordered — above every net a request waits under and its body's read gap,
which it would otherwise answer for, and inside the shutdown window
(`decisions/deadline-order.md`); a deployment past the window, or with the
read gap at or past the deadline, is told at
boot. **An edge's per-unit deadline is `<unit>_timeout` in its config**, read
from `<PREFIX>_<EDGE>__<UNIT>_TIMEOUT_SECS` with the unit's second segment —
`HTTP__REQUEST_`, `WS__MESSAGE_`, `MCP__OPERATION_`, never a mechanism
(`READ_TIMEOUT`) — on `nest_rs_http::UNIT_TIMEOUT_BOUNDS`. A unit
whose head leaves before its answer (a socket's message, an MCP operation on
an event stream) needs its own, since the request timeout ends at the head.
**A unit's deadline bounds its body; its commit is never cut, and a commit
that outruns its net is not retried**: each edge cuts the body it hands
`run_unit` at the deadline — HTTP's through the `RequestDeadline` its edge
hands the unit layer — and a cut unit is answered at its deadline whatever its
rollback still waits on; the driver settles on a task it drains at its close,
answers `Unhonoured::IN_DOUBT` past its settle net (`UnitOfWork::settle_net`,
above every statement bound) — and for a failed unit whose body was cut before
an atomic block's own commit answered — and waits for held work until the deadline; an
edge timer wrapping a unit is armed that net past the deadline, and answers in
doubt, never a retryable timeout — written once for the units no edge
deadline bounds, `run_unit_bounded`. **A command whose
answer is the only record of what it claimed is never cut** — a timeout does
not undo it — so it waits on the socket's liveness
instead (`queue.md`). **Every port method the framework awaits returns a
`Result`, and the port, never the backend, decides the fail-closed answer**:
a backend answers its error and logs nothing of its own. **Every such wait
goes through `nest_rs_core::within`**, which polls the call once bare and
contains its panic: a panic in a backend's call fails closed exactly as an
unanswered call does, logged once at `error` by the port
(`contained_panic!`). Past a net, on an error or a panic, every member
fails closed in its own terms: an occurrence claim skips the occurrence at
`warn`; a throttler hit denies the request for the window at `warn` naming the
store, never the key; a push or cancel is an error to its caller, a
checkpoint call fails the attempt as retryable and the worker retries its own
calls; `Strategy::authenticate` denies at `warn`; an indicator reads down —
its deadline is the orchestrator's probe, below every budget by design, so it
declares no net. **A resource the framework opens proves itself at boot through
`nest_rs_core::prove`**: what every attempt would meet refuses at once, the
rest is retried within the resource's budget, and the resource's crate says
each attempt on its own target, naming the endpoint, never a URL; a resource
exempt from the proof says why in its `//!`. **A resource the boot proves
ships its indicator in its own crate**, behind a `health` feature the umbrella
forwards weakly: the module opening the resource — imported by type, so it
runs once per app however many modules open the resource — attaches an
`IndicatorEntry` from its `register`, gated by construction, generic over the
instance where the resource is, on the readiness and startup probes, never
liveness, named for the crate subject owning the resource plus `:` and the
instance for a named one (`seaorm`, `redis`, `redis:cache`, `storage`), and
reading what the proof reads, one attempt per probe. An app importing
`HealthModule` so checks every resource it or its features opened and writes
nothing else; a health module it would import beside the resource is refused
(`.claude/decisions/resource-reports-its-own-health.md`; held by each resource
crate's `health` suites). A resource exempt from the proof ships none, for the
reason its `//!` gives: an outage that must not stop the boot — the OTLP
collector, an authorization server, a social provider — must not take every
replica out of rotation either. **A loop
retrying a backend waits `nest_rs_core::Backoff`'s curve** — exponential,
±20 % jitter drawn from a seed, gRPC's connection backoff — never a doubling
of its own: a reconnect loop seeds it with `process_seed()`, so replicas come
back apart; a loop its library owns (redis's `ConnectionManager`,
object_store) stays the library's.
**An outbound client the framework
opens follows one policy**, and is private to the crate whose protocol needs it
(OAuth's exchange, the JWK Set, the OTLP export, storage) — never an outbound
HTTP surface (`CLAUDE.md`, hard "no"): its TLS is `ClientTls`; it speaks
`https` unless its protocol's specification lets the URL declare plaintext
(OTLP's `http://`) or the deployment opts in (storage's `ALLOW_HTTP`), and TLS
material beside a plaintext URL is refused; it follows no redirect, but where
its library decides (object_store follows S3's); it carries its own connect and
total bounds, and a cap on what it reads into memory, refused on the declared
length and while streaming (the JWK Set's `JWKS_MAX_BYTES`, OAuth's
`ANSWER_MAX_BYTES`); and a sentence shows its URL only through
`endpoint_shown`. A body it streams is a transfer, bounded by its stall rather
than its total, so no size is cut (storage's `read_timeout` beside its
`operation_timeout`).

A worker job — a queue attempt, a tick — and an event listener are the units
no edge deadline bounds, so their decorator's `timeout` key is their deadline,
defaulting to `nest_rs_core::JOB_TIMEOUT` and never off: past it an attempt's
body is cut and fails retryably, a listener's is cut and its unit rolls back,
and each files `error` (held by `nest-rs-codegen`'s job table, `#[on_event]`'s
sibling grammar beside it, and each edge's suite). A listener's deadline starts
with its own call, so one queued behind a slow sibling is not charged for it.
Synchronous seams (`AbilityFactory`, the WS `Registry`, `ConfigSource`) are
outside the family.

## Lifecycle hooks and the way down

`#[hooks]` submits phase-tagged methods, and **init hooks run dependencies
first**: a host's hooks run after the hooks of every provider it injects, at
any depth, and shutdown hooks run in the exact reverse, so nothing is torn down
under a provider still using it. Ties follow module import order (imports
first, as declared), then `providers` order; a host no module lists (a seed, a
factory's output, a hand-written registration) runs before every listed one at
init and after them at shutdown; a host's own hooks run in the order its `impl`
declares them, the same both ways (across several `impl`s, by module path
first); one hook at a time. A provider orders its hooks after another's by
injecting it — the graph is the only order, never a name or a priority (held by
`nest-rs-core`'s lifecycle suite). Each hook is a unit of work,
`lifecycle.hook`, with a trace and a line of its own and the data context a
tick gets: one transaction through the unit of work the driver's binding
provides with `provide_unit_of_work`, committed on its `Ok`, bare with none
bound (held by `nest-rs-core`'s and `nest-rs-database`'s suites). The init phases run once per `App`, whether
`init` or `run` reaches them first; an init hook that fails or panics aborts the
boot naming it, then the shutdown phases run for every host and the resources
close, and the app refuses its init again (`InitFailedError`).
The shutdown phases run once every transport has stopped or been abandoned at
its bound, never at the signal (`decisions/shutdown-hooks-after-transports.md`,
held by `nest-rs-core`'s lifecycle suite): the app's state and stop cause are
`AppLifecycle`'s, the kernel alone moves them, and a loop a provider runs
beside the app stops at its `stopping()`.
Shutdown hooks are best-effort — a failure or panic is logged at
`error` and the next runs — and the three shutdown phases **share one budget**,
never a bound per hook (`k` stuck hooks would cost `k` bounds). A hook still
waiting when it is spent is abandoned at `warn` naming its module; every later
hook still starts and is polled once.

**No `static` holds a provider**: a per-site cache keeps `Weak` handles and
the container the strong ones (`SiteChains`), so what an app built goes with
it. **Nor does a provider hold the container**, or a provider holding it, past
the app: what needs one after the boot gives it back at its module's close
(`HealthService`'s container, `EventBus`'s listeners).

**A wait on the way down without a bound is a `SIGKILL` with nothing to say
why.** So the way down is bounded end to end, and what still runs at a bound is
abandoned with a line naming it, never awaited in silence. Its invariants — the
steps and their constants are tabulated on `SHUTDOWN_HOOKS_TIMEOUT`, the signal
and the teardown in `way_down.rs`'s `//!`:

- **No transport serves until every transport listens**: each takes what
  serving needs (a socket, its TLS material) in `Transport::listen`, after the
  init hooks, and a `listen` that fails is the boot's failure, before anything
  serves.
- **The transports stop together**, each within its `Transport::stop_bound`: a
  window for what it still runs, then one settle for what it stopped to unwind,
  so nothing it carried runs into the hooks; a unit blocking its thread cannot
  unwind and is named at `error`. **The kernel holds each transport to its
  bound**: a bound past `MAX_STOP_BOUND` is refused at the boot, and one still
  serving at the longest bound plus the settle is abandoned and named at
  `error`. **An early end stops the app**: a `serve` returning before the stop,
  `Ok` included, is a failure. What each edge owes is in `edges.md` and
  `queue.md`.
- **A resource the framework opens closes after the shutdown hooks**, in
  reverse, within their budget (`ContainerBuilder::provide_close`), and the
  container then drops off the runtime thread within what is left of it: a
  provider whose `Drop` blocks is abandoned with a line. Every end of an app —
  `run`, `close`, a failed init — shares this tail; a build that fails runs
  the closes, and what it built drops by ownership. Every unit has settled
  before a close runs, so what is still lent out then has no owner: a close
  waits for it `SHUTDOWN_SETTLE_TIMEOUT` at most and names what it leaves at
  `warn` (`decisions/pool-closes-at-the-settle.md`).
- **Developer code on the way down gets a hook's budget** — a scheduled tick
  still running gets it *before* the hooks, so no tick runs through the hooks it
  may depend on.
- **Telemetry flushes within a bound of the crate's own**, since the SDK's
  providers do not honour theirs.
- **The console never holds the app or its exit.** Under `#[nest_rs::main]` it
  is written through a bounded queue: a stalled stdout drops lines, counted
  and reported at `warn`, and every exit the kernel controls writes what is
  still queued within a bound of its own, the last on the way down.
- **The runtime is torn down by `#[nest_rs::main]`** within what the hooks left
  of their budget, so blocking work left behind is named and abandoned rather
  than holding the exit (no `#[tokio::main]`, held by `clippy.toml`).
- **A signal received on the way down exits at once**, after one `error` naming
  what it abandons; the handlers are installed by `#[nest_rs::main]` before
  `main` runs, and a signal during the boot stops it at its next await, naming
  the step.
- **The grace period holds the sum.** A test in `nest-rs-testing` sums every
  framework transport's `stop_bound`, the hooks' budget and both flushes under
  a Kubernetes pod's default grace, and fails the day one stops fitting.

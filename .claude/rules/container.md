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
  (providers built from seeds and factory outputs). `main` holds only
  `App::builder().module::<AppModule>()` and its transports; a synchronous app
  keeps `App::new`.
- **A factory comes in three kinds** — a default, a declaration, a dyn binding
  — each with or without the factory outputs it reads, written as one tuple
  (`FactoryInputs`, sealed): a new arity is an impl, never a new form.
- **A `register` refuses, never panics**: what it cannot build it files with
  `ContainerBuilder::refuse` and returns, as a factory returns `Err`. So does a
  site a transport composes at `configure` (a route, a gateway) against the
  built container: a mount has no `Result`, so it files the refusal, mounts
  what denies, and the boot fails on it once the transports are configured —
  `App::run` and the test harness read it
  (`.claude/decisions/site-refuses-through-its-container.md`). A factory
  is queued in `collect`, so a module or setup wiring another imports it in
  both phases, through `ContainerBuilder::import` — the one way into a
  module's phases, held by `Collecting` and `Registering`
  (`.claude/decisions/module-phases-by-import.md`); one queued later is
  refused (`LateFactoryError`), and a module imported in a collect alone
  (`UnregisteredModuleError`).
- **Providers are singletons unless scoped.** `scope = request` is built per
  request from the singleton root: it may inject singletons and other
  request-scoped providers (one instance each per request), never the reverse,
  and a handler reaches it through the edge's `Scoped<T>`. `scope = transient`
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
  dynamic imports are not deduplicated.
- **The container is flat and the access graph is the contract.** Visibility is
  Rust's: hide an implementation module-private and bind a `pub trait` with
  `Foo as dyn Trait`; there is no `exports` list. The binding takes a singleton
  only (a `const` assertion on its residency) and holds that one `Foo` under
  its own type and each trait it is listed with, so a listing names a provider
  once. `#[module]` records each provider's injected types, and the boot fails
  (`AccessGraphError`) on a provider injecting what its module neither owns,
  imports transitively, nor receives as global infrastructure. It governs
  `#[inject]`, optional or not, and the `#[use_*]` layer lists — an optional
  dependency no module the app reaches provides is absent, never a leak — and
  `Container::get` at runtime is an unchecked escape hatch. **A provider a module lists is built, or the boot
  refuses naming why** (`UnbuiltProviderError`, after the access graph): an
  import cycle is tolerated while no provider waits across it. One built
  without an optional dependency the boot registers afterwards would run as if
  it were absent, and is refused too (`LateOptionalDependencyError`).

## `for_root` — one seam, one value, no chain

A module is configured in exactly one place, by exactly one value:
`Module::for_root(x)`, where `x` carries everything the app declares about it.
The `*Setup` it returns is opaque — no public method, no second constructor on
the module type. A declaration that does not fit makes `x` grow a **field**,
never the seam a **method**. Held by review: no inherent `impl` on a `*Setup`,
and no inherent `pub fn` on a module type besides `for_root`.
`ConfigModule` is the one carve-out — its `for_root` / `for_feature` /
`provide_feature` / `setup` / `resolved` are the primitives every other seam is
built from, and `provide_feature` and `resolved`, the read a `register` or a
factory makes of the config, are public for third-party drivers. A seam one framework
crate needs from another lives in its crate's `__private` (`macros.md`); one
only its own crate needs stays private.

`x` has two shapes and only two:

- **`impl Into<Option<C>>`**, `C` the module's `#[config]` — the default.
  `None` is the environment over `C::defaults()`; `Some(c)` is the environment
  over `c`, per field.
- **`impl Into<MOptions>`**, a plain `Default` struct `{ config: Option<C>, … }`
  beside the setup — only when the module carries a declaration with **no
  environment twin** (MCP's identity). It keeps `From<C>` and `From<Option<C>>`
  so the config-only call reads like every other module's, and the `Option` is
  load-bearing: flattening it demotes the `.env` cascade below the pin.

A setup that only pins its config is `ConfigSetup<M, C>` behind a named alias,
built by `ConfigModule::setup`; a hand-written setup is for one whose `collect`
queues more than the config or whose `register` does more than recurse.

- **A bare import is a dependency declaration**, not a second seam. A module
  that must not be imported bare hides its `#[module]` behind a private host
  struct and exposes only the façade (`OAuthResourceHost` /
  `OAuthResourceModule`).
- **Who gets a `for_root`, and `for_feature`'s role, are `architecture.md`'s**
  (*Configuration*): every `nest-rs-*` module owning a `#[config]`, never one
  owning none (`.claude/decisions/config-one-seam.md`).
- **A value an import site chose is a declaration**
  (`ContainerBuilder::provide_declared_factory`, carrying its remedy sentence),
  and so is the trait object a `provide_factory_dyn` binds.
  The boot outcomes `architecture.md` lists — supersede,
  `ContestedDeclarationError` naming both imports and their positions before
  any factory runs, `UnresolvedFactoryError` — each have a behaviour test in
  `nest-rs-config`. We
  exceed NestJS here on purpose: a silently dropped declaration is a swallowed
  error.
- **One exception: `OpenTelemetry::init` / `init_with(config)`** belongs to `main`,
  because the global tracer must exist before any module registers and its
  guard's `Drop` flushes. Any other claimed exception is reported, not written.

## Configuration — the loader loads, the consumer judges

`nest-rs-config` resolves each variable through the tiers — deployment, the
`.env` cascade, defaults — except that a config pinned in code switches its
whole namespace to deployment-over-pin, and the cascade is not read beside a
pin. It hands values over, assigns them no meaning, and never arbitrates between
variables or tiers.

- **`<KEY>_FILE` is a spelling of `<KEY>`, never a second variable**, and every
  namespaced variable has it (the kernel's pre-container `<PREFIX>_LOG*` and
  `<PREFIX>_ENV` do not), so a secret never has to sit in the process
  environment. Every file is read through the one bounded reader
  (`read_material`), and a consumer that re-reads on renewal reuses it. The
  deployment chooses the spelling: either one present in the deployment tier
  shadows both in `.env`; both set in the answering tier is refused naming both
  — the loader's one refusal, about spelling, never meaning. A refusal never
  repeats a value read from a file.
- **A combination of variables is the consumer's**, refused at boot in the one
  constructor every path reaches — config-driven or built in code — naming what
  is set, never settled by picking one.
- **A `#[config]` type's `Default` is the production-safe value, and an
  undeclared environment is production**: a development convenience — a
  credential the dev container accepts, plain HTTP, a public surface — lives
  in `Config::defaults()`, armed only by a declared development or test
  profile (`Environment::is_deployed` is the one test), never in `Default`, so
  a pin over `..Default::default()` opens nothing in any profile and neither
  does a process that forgot `<PREFIX>_ENV`. Held by each member's unit test,
  and review.
- **A duration a deployment sets has a floor and a ceiling**, refused naming the
  variable whether pinned or read, never clamped. The floor is where the thing
  stops working; the ceiling is where a value becomes a slip, **and never above
  what the library or kernel it reaches accepts** — no value the boot accepts may
  panic or be refused below it. `0` is off only where the declaration says so
  (`Floor::UnitsOrOff`); a pinned `Some(Duration::ZERO)` is refused, since off in
  code is `None`. Held by the `DurationBounds` type, whose rustdoc has the
  mechanics, including the hand-built path through `DurationBounds::check`.
- **A variable no config claims is reported, never ignored** — at `warn`, once,
  by name and never by value: a key under a namespace this binary read that no
  config read, and a near miss of such a namespace. Everything else is silent by
  design: one `.env` serves several binaries, and a namespace this binary does
  not read is another binary's. The near-miss tests are narrow for that reason
  (the algorithm is `unclaimed.rs`'s `//!`); a diagnostic that fires on correct
  configuration teaches operators to filter its target out. *One namespace, one
  type* (`architecture.md`) is refused at the read as
  `ConfigError::SharedNamespace`.
- **`nestrs doctor` links no framework crate**, so its reading of a variable is a
  second implementation of the loader's, held to it by a differential test in
  `nest-rs-cli`'s suite; it does not run the unclaimed report.
- **A structured `#[config]` value is a payload**: decoded by
  `ConfigService::json`, never by a config's own `serde_json`, and redacted at
  the `ConfigError` sink (`CLAUDE.md`, no payload value in an error).
- **A TLS client's material is `ClientTls`**, read under its namespace's
  `TLS_CA_CERT`, `TLS_CERT` and `TLS_KEY`: the one client vocabulary lives
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
graph's set for the running root; linked but unreachable is inert. The gate is
always the entry's owner: an entry naming a DI provider is gated by that
provider's reachability; an entry naming none (`SocialProviderEntry`) by the
module providing its registry — which is why such an entry needs no module of
its own. **Metadata attached from `Discoverable::register` is gated by
construction**, since `register` runs only for an imported module's provider;
never bolt a `ReachableProviders` filter onto it for symmetry. **Being buildable
is not discovery**: a discovered entry's own config decides its fate — complete
is active, absent is inert with a boot `warn`, partial or invalid fails the boot
naming it. **A registry the app's code reads is filled by `provide_wiring`,
before any hook — never by a lifecycle hook**: the kernel runs every wiring
once, in registration order, on the sealed container, and a hook that ran first
would read the registry empty. A wiring is synchronous, so no I/O slows the
boot there; one that fails ends it with `WiringFailedError` naming the registry.

**An inert host is reported at the level its cause earns**, and the boot reads
the cause (`InertHost`, from the module descriptors and the app's
`Composition`). The app's own code — a host in its roots' crates that no
imported module lists, one registered outside every imported module — is a
`warn` with `cause` and a `hint` naming that cause's remedy. A dependency's
host the app neither imports nor registers — the framework's, a package's, a
shared library crate's — is said at `debug` (`cause = "dependency"`). The cause
is read off the composition, never off a crate's name: a prefix is no evidence
of who wrote a crate. Three disciplines bind every boot diagnostic:

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
sentence is a closed door that looks open. The unit is the **port**, never the
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
  a `module.rs`, and it may name the first-party binding as the remedy. *By type
  parameter* (`AuthnGuard<S: Strategy>`): two instantiations are two types, so
  no arbitration is owed. *By configuration*: the import opens the gate and
  configuration decides which members are active, zero to all.

**A port promises only what every backend it ships holds**; a looser backend
either does not declare the capability, or the port is worded loosely enough and
the backend's own bound is written on the backend. **A port owns a config
namespace if and only if its contract requires the integrator to honour a
config** (the throttler's does); otherwise the adapter's config takes the
namespace its path gives it (`architecture.md`) —
`.claude/decisions/port-config-namespace.md`.

**An open-ended library is a public behavioural contract plus inventory
discovery**, and `nest-rs-social` is the template. `SocialProvider` owns the
flow: `authorize` / `exchange` default to the shared PKCE/CSRF flow through the
provider's `OAuthClient`, so a standard provider implements `profile` and a
non-standard one overrides a step without changing the trait. A social provider
is not a DI provider — reached only through `SocialRegistry` — so it has no
module; a third-party provider crate is `config.rs` plus `provider.rs`. A
duplicate key, or a registry key disagreeing with the provider's `key()`, fails
the boot. Keyed injection (`#[inject(key)]`) stays the tool for static roles
such as primary and replica pools.

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
boot. **A command whose answer is the only record of what it claimed is
never cut** — a timeout does not undo it — so it waits on the socket's liveness
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
declares no net. **A loop retrying a backend waits `nest_rs_core::Backoff`'s
curve** — exponential, ±20 % jitter drawn from a seed, gRPC's connection
backoff — never a doubling of its own: a reconnect loop seeds it with
`process_seed()`, so replicas come back apart; a loop its library owns
(redis's `ConnectionManager`, object_store) stays the library's.
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

A worker job — a queue attempt, a tick — is the one unit no edge deadline
bounds, so its decorator's `timeout` key is its deadline, defaulting to
`nest_rs_worker::JOB_TIMEOUT` and never off: past it the attempt is cut, fails
retryably and files `error` (held by `nest_rs_codegen::job`'s table and each
edge's suite). Synchronous seams (`AbilityFactory`, the WS `Registry`,
`ConfigSource`) and in-process listeners are outside the family.

## Lifecycle hooks and the way down

`#[hooks]` submits phase-tagged methods, and **init hooks run dependencies
first**: a host's hooks run after the hooks of every provider it injects, at
any depth, and shutdown hooks run in the exact reverse, so nothing is torn down
under a provider still using it. Ties follow module import order (imports
first, as declared), then `providers` order; a host no module lists (a seed, a
factory's output, a hand-written registration) runs before every listed one at
init and after them at shutdown; a host's own hooks run in the order its `impl`
declares them, the same both ways (across several `impl`s, by module path
first); one hook at a time. A provider orders its hooks after another's
by injecting it — the graph is the only order, never a name or a priority
(held by `nest-rs-core`'s lifecycle suite). The init phases run once per `App`, whether
`init` or `run` reaches them first; an init hook that fails or panics aborts the
boot naming it, and the app refuses its init again (`InitFailedError`).
The app's state and stop cause are `AppLifecycle`'s; the kernel alone moves
them, and a loop a provider runs beside the app stops at its `stopping()`.
Shutdown hooks are best-effort — a failure or panic is logged at
`error` and the next runs — and the three shutdown phases **share one budget**,
never a bound per hook (`k` stuck hooks would cost `k` bounds). A hook still
waiting when it is spent is abandoned at `warn` naming its module; every later
hook still starts and is polled once.

**No `static` holds a provider**: a per-site cache keeps `Weak` handles and
the container the strong ones (`SiteChains`), so what an app built goes with
it.

**A wait on the way down without a bound is a `SIGKILL` with nothing to say
why.** So the way down is bounded end to end, and what still runs at a bound is
abandoned with a line naming it, never awaited in silence. Its invariants — the
steps and their constants are tabulated on `SHUTDOWN_HOOKS_TIMEOUT`, the signal
and the teardown in `way_down.rs`'s `//!`:

- **The transports stop together**, each within its `Transport::stop_bound`: a
  window for what it still runs, then a settle for what it stopped to unwind,
  so nothing it carried runs into the hooks; a unit blocking its thread cannot
  unwind and is named at `error`. What each edge owes is in `edges.md` and
  `queue.md`.
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

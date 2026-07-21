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
a companion `*-macros` crate re-exported by its home crate. Shared token
helpers in `nest-rs-codegen`. A `*-macros` crate **must not** depend on
its surface crate — emit absolute-path tokens (`::nest_rs_core::*`,
`::std::sync::Arc`); never rely on call-site scope.

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
  provider's own `key()`, **fails boot**. Pinning config in code is the
  ordinary config seam (provide the `GithubSocialConfig` value), not a
  second module. A third-party provider crate is therefore exactly two
  files: `config.rs` (`#[config]` + `SocialProviderConfig`) and
  `provider.rs` (`SocialProvider` + `inventory::submit!` whose `build` is
  one `resolve_provider` call). Ships first-party GitHub + Google;
  third-party provider crates are **encouraged** through the same public
  seam. Keyed injection (`#[inject(key)]`) stays the tool for **static,
  compile-time roles** (primary/replica pools).

  **This extension-crate posture — a public behavioral contract +
  inventory discovery — is the template for any future open-ended
  library in the repo.**

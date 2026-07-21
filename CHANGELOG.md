# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2026-07-21

The first stable release. Every crate ships at `1.0.0` in lockstep, and from
here the public API follows semver: a breaking change waits for `2.0`.

### Third-party surface, frozen for the 1.x line

A handful of crates *are* the framework's public surface — their types appear
in signatures the macros emit. Their majors are tied to the nestrs major and
are frozen within 1.x: `poem = "3"`, `sea-orm = "=2.0"`,
`async-graphql = "=7.2.1"`, `rmcp = "2.2"`, `inventory = "0.3"`,
`validator = "0.20"`, `schemars = "1"`. sea-orm and async-graphql are
exact-pinned (not caret) because the ORM bounds and the GraphQL registry
codegen read enough of their surface that even a *minor* can shift generated
code.

### Changed

- **`DatabaseConfig::retry_serialization_conflicts` is now
  `observe_serialization_conflicts`** (env
  `NESTRS_DATABASE__OBSERVE_SERIALIZATION_CONFLICTS`). The flag never retried
  — it tags a commit-time conflict (`40001` / `40P01` / `1213` / `1205`) as a
  structured `warn` on `nest_rs::orm` so contention is distinguishable from a
  generic commit error. The old name promised a transparency the framework
  deliberately does not offer: replaying a conflict means re-running the whole
  handler, and a handler may already have pushed a job, emitted an event, or
  written an object — none of which roll back with the transaction. Retrying
  stays the service's decision, at a boundary it knows is replayable
  (`nest_rs_seaorm::retry::retry_on_conflict`). Renamed before the freeze
  because a config key is public surface for the whole `1.x` line.

- **`AuthGuard` is now `AuthnGuard`** (`nest_rs_authn::AuthnGuard<S>`). It was
  the only half of the pair not carrying its concern's suffix, so
  `#[use_guards(AuthGuard, AuthzGuard)]` read as if the two guards answered
  different kinds of question. They don't: one establishes *who* (authn), the
  other *what they may do* (authz). Rename the import; nothing else changes.
  `OAuthGuard` and other `OAuth*` names are untouched — that `Auth` is OAuth's.

- **Social providers activate from configuration, not from a per-provider
  module import.** Importing `SocialModule` is now the whole wiring step: it
  owns every registry entry, so it is the module gate, and inside that gate
  each linked provider turns on when its credentials are set. A provider with
  no credentials is inert with one boot `warn` (its routes `404` like an
  unknown key); a *partially* set or invalid one **fails boot naming the
  provider**, so a half-configured login is never silently dropped.
  - `GithubSocialProviderModule` / `GoogleSocialProviderModule` and their
    `Setup` types are **removed**. Delete those imports; pin config the
    ordinary way by providing a `GithubSocialConfig` / `GoogleSocialConfig`
    value, which still wins over the environment.
  - `SocialProviders` is renamed **`SocialRegistry`** — it is the registry, not
    the providers.
  - `SocialProviderEntry` gains `env_namespace` and `build` (normally one
    `resolve_provider` call) and drops `provider_type_id` / `resolve`. A
    third-party provider crate is now two files, `config.rs` + `provider.rs`,
    with no module to write: a social provider is never `#[inject]`ed by type,
    so it has nothing for a module of its own to own.

### Added

- **Ambient request state now reaches an MCP tool body — `Repo` works on MCP.**
  rmcp dispatches every tool call on its own spawned task, so the request
  scope, ambient executor and ambient ability installed around the endpoint
  never reached a tool. The new `PropagatingHandler` closes that gap: the
  endpoint stashes the state in the HTTP request extensions, rmcp forwards them
  as `http::request::Parts` into the operation's `RequestContext`, and the
  handler re-installs them *inside* the dispatch. A tool method now resolves
  `Scoped<T>` and reads through `Repo` with the caller's row filter applied —
  the same transparency HTTP and GraphQL have, with no filtering written in the
  tool.
  - New `McpToolContext` seam (`nest-rs-mcp`) with the first-party
    `nest_rs_seaorm::McpDataContext` behind seaorm's new `mcp` feature —
    the MCP twin of `WsDataContext`. It installs a **lazy** per-operation
    transaction: a read-only tool opens none, a writing tool commits on success
    and rolls back on error. `AuthzMcpModule` provides it.
  - Without a registered `McpToolContext` a `Repo`-backed tool still fails
    **closed and loud**, never unscoped.
  - `endpoint_with_guard` takes the context as a second argument (the `#[mcp]`
    macro resolves it from the container; hand-written call sites pass `None`).

- **MCP reaches the security sub-layer through the same wiring as GraphQL.**
  Both transports are `EdgePosture::Exempt` and gate in-band, but only
  `/graphql` had the surrounding seams; `/mcp` now has all of them, so the two
  answer identically to one app wiring.
  - **The global guard pool reaches `/mcp`.** With no `dyn McpOperationGuard`
    registered, the endpoint folds the `use_guards_global(...)` chain in-band
    (`FallbackMcpGuard` + `nest_rs_guards::GlobalPoolMcpGuard`, behind guards'
    new `mcp` feature) instead of going straight to deny-all. A global
    `ThrottlerGuard` now rate-limits a tool call — it previously could not.
    The fallback only ever *widens* what the app declared: with no pool (or an
    empty one) `/mcp` stays deny-all, and unlike `/graphql` it carries no
    `Public` marker, so a pooled `AuthnGuard` still refuses an anonymous call.
  - **`McpOperationGuard` gained `capture` + `around`** (both defaulted, so
    existing impls are unaffected): snapshot on the request, install *inside*
    rmcp's spawned dispatch — the same split `McpToolContext` already used for
    the same crossing. `McpAbilityBridge` implements them, so the **guard**
    installs the caller's ambient `Ability` on both transports and a tool body
    is now scoped even when the app registers no `McpToolContext`.
  - **One authn→authz chain.** `nest_rs_authz::run_ability_chain` holds the
    ordering once; each bridge only maps the resulting `Denial` into its own
    transport error. Side effect: an MCP denial keeps its status, so a `429`
    from a throttler in the chain reaches the client as `429` with its
    `Retry-After` instead of a flattened `401`.

- **Three test suites that never ran now run.** `cargo nextest run --workspace`
  builds every member with its *default* features, which silently excluded a
  large part of the framework's own coverage: `nest-rs-authz`'s http / graphql /
  mcp bridge tests compiled away behind `#[cfg(feature = …)]`, and the
  `nest-rs-seaorm` and `nest-rs-redis` e2e targets were skipped outright
  because their `required-features` were unsatisfied. Each crate now carries a
  path-only **self dev-dependency** that turns its own features on for its test
  targets (dev-deps do not propagate, and Cargo strips them from the published
  manifest). The workspace-wide `-E 'binary(e2e)'` step went from 1 test to 21.
  - Enabling them surfaced two real defects, both fixed: the `nest-rs-seaorm`
    e2e harness `expect`ed `NESTRS_DATABASE__URL` instead of defaulting to the
    dev container like its `nest-rs-redis` / `nest-rs-storage` siblings, and its
    shared probe tables were guarded by a per-*process* `OnceCell` while nextest
    runs each test in its own process — so a fresh database raced
    `CREATE TABLE IF NOT EXISTS` against the Postgres catalog. The DDL now
    serializes on a transaction advisory lock.

- **Two macro diagnostics are pinned by compile-fail snapshots.** Arming the
  `#[routes]` response shaper with a type that only *borrows* the
  `Authorize`/`Bind` name now has a `trybuild` fixture, as does a
  `for_root(...)` value that is not `Send` (the bound `#[module]`'s
  construct-once dynamic imports introduced). Both errors were already
  emitted; neither was guarded against silent regression.

### Removed

- **`nest_rs_authz::mcp::masked_output`.** It was a one-line delegation to
  `nest_rs_authz::masked_output_ambient` — two public names for one behaviour,
  against *one way to do a thing*. Call `masked_output_ambient` directly; the
  signature and the fail-closed semantics are unchanged.

- **The unfinished offset-pagination surface.** `PageArgs`, the `<Name>Page`
  envelope emitted by `#[expose(..., paginate)]`, the `paginate` flag itself,
  and the `paginate = page` mode of `#[crud]` are all gone. The mode was never
  wired — both transports answered it with a compile error — so the types
  documented a capability the framework refused to generate. Keyset
  (`paginate = cursor`, the default) and `paginate = none` are the whole knob;
  a consumer that genuinely needs page numbers plus a total hand-writes that
  operation on its service. No caller in either workspace was affected.

### Known for the 1.x line

- **`Guard::check_http` sits on the base `Guard` trait**, so `nest-rs-guards`
  depends unconditionally on `poem` and `nest-rs-http` — a queue-only binary
  still compiles the HTTP stack. Build hygiene, with no runtime, security or
  correctness effect; moving it to an extension trait touches every guard impl,
  HTTP dispatch and the boot chain-validation, so it lands in `2.0`.

## [0.5.0] - 2026-07-19

### Changed

- **WS message handlers are transactional.** `WsDataContext` installs the
  same lazy executor per message: a read-only or non-querying message opens
  no transaction, while a writing handler commits on a success reply and
  rolls back on an error reply — a multi-write handler that fails mid-way no
  longer half-persists. Guest connections stay fail-closed (deny-all without
  an ambient ability).
- **Mutating HTTP requests no longer pay `BEGIN`/`ROLLBACK` before guards
  run.** `DbContext` now installs a lazy executor (`Executor::Lazy`): the
  request transaction opens on the **first data-layer touch**, so a
  guard-denied POST — or any mutating request that never queries — opens
  zero transactions and consumes no Postgres transaction slot. Commit /
  rollback semantics, the `MappedError` rollback, and the escaped-executor
  fail-loud check are unchanged.
- **`Creatable::create` is atomic on every executor shape.** On a pool
  executor (a WS message handler, a bare `with_executor`) the insert and its
  SQL scope re-check now run in a local transaction — an out-of-scope create
  surfaces `RecordNotInserted` and persists nothing, instead of relying on
  the HTTP request transaction for the rollback.
- **`ThrottlerStore::hit` is async.** The Redis store awaits its round-trip
  on the request task instead of parking a runtime worker with
  `block_in_place` + `block_on` per rate-limit check (which also panicked on
  a current-thread runtime). Fail-closed behavior on a Redis outage is
  unchanged.
- **Guard chains are validated at boot from declared markers.** `Guard` gains
  `phase()` (authentication / authorization / other) and
  `produced_principal()` / `expected_principal()`. A chain listing authz
  before authn, or pairing an `AuthGuard` whose principal type differs from
  the `AbilityGuard`'s expected actor, now **fails boot with a named error**
  instead of answering 500 on every request; the old name-substring ordering
  heuristic is gone.

- **Response masking is cross-checked at run time.** `#[routes]` arms the
  response shaper by matching the `Authorize`/`Bind` parameter-type name; a
  renamed import (`use Authorize as Az`) used to disarm masking silently.
  Unarmed routes now carry a `MaskProbe`: when a masking extractor runs on a
  route whose shaper is not armed, the request fails closed with a logged
  `500` instead of shipping an unmasked body.
- **`Bind` / GraphQL `bind` no longer echo `DbErr` text to the client.** A
  failed by-id load logs the full error at `error` on `nest_rs::orm` and
  answers with an empty `500` (HTTP) / a generic `INTERNAL_SERVER_ERROR`
  extension (GraphQL), matching the `#[crud]` write mapper.

### Added

- **`nest_rs_authz::masked_reply`** — mask a handler's wire JSON with the
  ambient ability in one call, for surfaces with no automatic response
  shaper (a WS gateway reply, a hand-built payload). Same fail-closed core
  as the HTTP shaper and the GraphQL wrapper; the reference `users` WS
  gateway now uses it instead of ten hand-rolled masking lines.
- **`Creatable::create_from_active`** — insert a *prepared* `ActiveModel`
  through the same audited create path as `Creatable::create` (atomic
  insert + SQL scope re-check), for service methods that stamp server-side
  columns (the token's org id, a status default) before insert. The demo's
  users/posts services now use it instead of raw
  `ActiveModel::insert(&Repo::conn()?)`.

### Removed

- **Reserved cross-transport layer seams that were never invoked.**
  `Interceptor::wrap_graphql`/`wrap_ws` (with `GraphqlNext`/`WsNext`),
  `ExceptionFilter::catch_graphql`/`catch_ws`, and
  `Filter::filter_graphql`/`filter_ws` compiled but no macro or dispatcher
  ever called them — implementing one was a silent no-op. They are removed
  from the trait surfaces (along with the now-empty `graphql`/`ws` features
  of `nest-rs-interceptors`, `nest-rs-exception-filters`, and
  `nest-rs-filters`) until real wiring ships. Guards' cross-transport
  entries are unaffected; a global interceptor/filter still covers GraphQL
  and WS through the HTTP transport edge.

## [0.4.0] - 2026-07-19

### Changed

- **One error format at the HTTP boundary — RFC 9457
  `application/problem+json` everywhere.** Three shapes previously
  coexisted: the NestJS-style `{statusCode, error, message, details}`
  validation body, bare-text framework/service errors, and poem's
  plain-text transport errors (an unmounted-route `404`, a `413`). All
  now render as `ProblemDetails` (`type`/`title`/`status`, optional
  `detail`). Field-level validation errors ride as the RFC-9457
  **extension member** `errors`; `ServiceError`, guard denials
  (401/403/429, `Retry-After` preserved) and pipe rejections all map to
  the same envelope. `HttpTransport` installs a transport-edge boundary
  (`nest_rs_http::normalize_error_response`) that lifts any leftover
  raw plain-text error onto `problem+json` — a `Filter`/`ExceptionFilter`
  mapping (tagged `MappedError`) or a deliberately-typed body is left
  untouched, and internal (`5xx`) detail is dropped so no driver message
  reaches the wire. New `ProblemDetails::from_status` /
  `with_extension`.

### Added

- **The OpenAPI document is complete.** Previously skeletal — no query
  parameters, every path parameter a bare `string`, no security scheme,
  a lone `200` per operation. The generated document now carries: path
  parameters typed from the handler's `Path<T>` (a `Path<Uuid>` id is
  `string`/`format: uuid`), each `Query<T>` payload expanded into one
  query parameter per property (the `#[crud]` list op's `first`/`after`
  cursor is documented), a `bearerAuth` security scheme applied to
  guarded non-`#[public]` routes — including routes covered only by a
  `use_guards_global` pool — and per-route RFC 9457 error responses
  (401/403/404/409/422, each honest to what the route can produce)
  referencing a shared `ProblemDetails` schema. A new
  `NESTRS_OPENAPI__EMIT_DOCUMENT`/`DOCUMENT_PATH` config writes the
  document to disk at boot, the OpenAPI analogue of the GraphQL SDL
  emit, so a committed `openapi.json` stays fresh as a side effect of a
  dev run.

- **`HttpConfig.compression`** negotiates response compression (gzip /
  deflate / brotli / zstd) from each request's `Accept-Encoding` — one
  flag (`NESTRS_HTTP__COMPRESSION` or the pinned struct), off by default
  so a fronting proxy keeps ownership when it has it. A preflight
  (`OPTIONS`, no body) and an already-encoded response are left alone.

- **`Storage::get_stream`** downloads an object as a chunked byte stream
  instead of buffering the whole body ([`get_bytes`] collects), so a
  large media file flows to the client without ever sitting whole in
  process memory — feed it straight into a streamed HTTP body.

- **Streaming and multipart HTTP** are now first-class: poem's `sse`,
  `multipart` and `compression` features are enabled, so a handler can
  return `poem::web::sse::SSE` or a `Body::from_bytes_stream` response,
  or take a `poem::web::Multipart` upload, and `#[routes]` passes each
  through untouched. The demo's `audio` slice exercises all three
  (direct upload, streamed download, an SSE progress feed).

- **`nestrs g migration <name>`** scaffolds a SeaORM migration and
  registers it in **both** `crates/migrations/src/lib.rs` and
  `migrator.rs` — the `migrator.rs` vec is regenerated from the module
  list, so the two registrations can never drift (the one you forget by
  hand is the one that silently never runs).

- **`nestrs g resource --guarded`** scaffolds the hardened `#[crud]` +
  guards form (the `orgs/` shape) instead of the unguarded stub, for a
  workspace that already provides `AuthGuard` / `AuthzGuard` /
  `AuthzHttpModule`.

### Fixed

- **A duplicated concrete provider fails the boot.** Two modules (or a
  seed and a module) registering the same concrete type previously
  warned and silently last-write-wins — a wiring mistake that only
  surfaced as wrong behaviour. It now fails the boot with a named
  `DuplicateProviderError`, uniform with the other wiring checks. Keyed
  providers keep their documented last-write-wins, and `dyn Trait`
  bindings stay the intended override mechanism.

- **A missing `Ctx<T>` replies with a bare 500, not the Rust type.** The
  extractor built the response body from the internal Rust type name;
  that detail now goes to the logs and the client gets a bare 500.

- **A malformed relational rule fails ability construction instead of
  going fail-open.** `PredicateBuilder::related` rejects an invalid
  relation (composite key, or a relation not pointing at the declared
  related entity) with the `Deny` sentinel. In a `cannot(...)` that
  sentinel lowered to `1 = 0` and combined as `grant AND NOT(1 = 0)` —
  i.e. the restriction evaporated (fail-*open*). `AbilityBuilder::build`
  now returns `Result<Ability, MalformedRuleError>` and fails naming the
  faulty rule; the HTTP ability guard denies the request (fail-closed)
  when construction fails. A malformed grant, previously a silent
  deny-all, is surfaced the same way.

- **A scoped/transient provider's missing dependency fails the boot,
  not the first request.** The access graph only flagged *cross-module*
  reaches; a request-scoped or transient provider whose dependency was
  provided by no module at all passed boot and panicked at its first
  `get(...)` resolution — a runtime panic in place of the framework's
  hallmark named boot diagnostic. Lazily-built providers now report the
  names of what they inject, and the access-graph pass fails boot with a
  `MissingDependencyError` naming both the provider and the unmet
  dependency. A dependency provided imperatively (a hand-written
  `impl Module`) or by a lazy factory is still tolerated: the pass
  consults the actual registered set before declaring a dependency unmet.

- **An eagerly-built provider's missing dependency no longer panics
  before the graph check.** The synchronous register phase ran ahead of
  `validate_from_inventory`, so a missing dependency panicked with the
  generated `expect` message and preempted the named `AccessGraphError`.
  Construction now defers the miss to the graph pass, which reports the
  same unified `MissingDependencyError`; a genuine dependency cycle still
  panics with its cycle diagnostic naming the chain.

- **`#[crud]` writes return the right HTTP status.** A generated create
  / update / delete previously mapped every write failure to a blanket
  `500`, so a unique-constraint violation, an out-of-scope create the
  ability re-check rolled back, or a row that vanished mid-request all
  read as internal errors. The generated handlers now map a
  `DbErr` to its status — unique violation → `409`, `RecordNotInserted`
  → `403`, `RecordNotUpdated` / `RecordNotFound` → `404` — and a
  genuinely unexpected error to a `500` with an empty body (the driver
  message no longer leaks). A service with a manual create maps the
  unique violation to `ServiceError::conflict` for the same result.

- **Auto-resolved `has_many` relations are memory-bounded.** An
  `#[expose]`d `has_many` field's dataloader previously loaded *every*
  child of a parent (`.all()` with no `LIMIT`), so a relation with large
  fanout (`Org.posts` over millions of rows) could pull an unbounded
  result set into memory. The generated FK loader now caps its batch
  query at `RELATION_LOAD_CAP × keys` and truncates each parent's bucket
  to `RELATION_LOAD_CAP` (100), logging a `warn` when it does. A relation
  that legitimately exceeds the cap should be a paginated
  `#[field_resolver]`, not an auto-resolved list.

## [0.3.0] - 2026-07-16

### Added

- **Social login with an open provider contract.** The new
  `nest-rs-social` crate makes social login a first-class capability.
  `SocialProvider` is flow-owning — `authorize` / `exchange` default to
  the shared PKCE/CSRF flow, so a standard provider implements only
  `profile`, while a deviating one (Apple's ES256 client secret)
  overrides a step without changing the trait. Ships first-party GitHub
  and Google; a third party publishes their own provider as an
  independent crate through the same seam. Discovery is link-time and
  module-gated: an unimported provider stays inert with a boot warn, and
  a duplicate or disagreeing key fails boot rather than silently
  shadowing a login provider. Identity keys on the provider's stable
  `(provider, subject)` pair, not the email, so a user who changes their
  provider email keeps their account.
- **Keyed providers.** `#[inject(key = "…")]` fields and `provide_keyed`
  let several instances of one concrete type coexist under a
  `ProviderKey`. The access graph validates each keyed dependency
  against the global keyed set at boot, naming both type and key on
  failure. Used by the keyed OAuth clients behind social login.
- **Request-scoped providers inside GraphQL and MCP handlers.**
  `nest_rs_graphql::Scoped<T>` and `nest_rs_mcp::Scoped<T>` resolve an
  `#[injectable(scope = request)]` provider from inside a resolver or
  tool body, falling through to singletons — so both transports share
  the per-request resolution model HTTP already had.
- **Type-safe queue identity.** `#[queue(name = "…", job = …)]` declares
  a `QueueName` unit struct carrying both the wire name and the job
  type. Both sides name the type (`push_to::<Q>`,
  `#[process(queue = Q)]`) and the macro asserts the process method's
  job argument matches, so a typo is a compile error instead of a job
  that silently never drains. The stringly-typed form still works.
- **Redis-backed throttler.** `RedisThrottler` puts the fixed-window
  counter in Redis so N replicas share one budget per client instead of
  N× the limit. The window advances in a single atomic Lua script (one
  round-trip, no check-then-act race) and fails closed on a backend
  outage.
- **Per-argument pipes on every transport.** `Piped<P, T>` / `Valid<T>`
  bind on GraphQL, WebSockets, and queue handlers (value-form carriers in
  `nest-rs-pipes`, stripped by `#[resolver]` / `#[messages]` /
  `#[processor]`); HTTP keeps its extractor forms. A rejection surfaces as
  the transport's native error (GraphQL error, WS error frame, job error).
- **Relational predicate scoping.** `p.related::<R, _>(relation, |r| ...)`
  scopes an entity by a condition on a related entity through a typed
  SeaORM relation — lowered to a semi-join (`IN` subquery / correlated
  `EXISTS`), with boot-time guards on the relation target and key arity.
- **Scalar predicate variants.** `p.ne` / `p.lt` / `p.lte` / `p.gt` /
  `p.gte` (`Cmp`) and `p.is_null` / `p.is_not_null` (`IsNull`).
- **Action-typed authorization proofs.** `Authorized<E, A>` carries the
  action as a type parameter, with `bind_required::<S, A>` as the GraphQL
  subject binder — a `Read` proof no longer passes where an `Update` proof
  is required.
- **Generic client-credentials grant helper** in `nest-rs-authn`.
- **Selective `#[crud]` ops with segregated write traits.**
  `ops = [list, get, delete]` synthesises exactly those; the write half
  lives in opt-in `Creatable` / `Updatable` / `Deletable` traits, so a
  read-only resource declares no placeholder input types.
- **Generated list operations paginate by default**, with a hard
  backstop on page size.
- **`ServiceError` carries real 4xx variants** plus `Internal` — features
  stop redefining plumbing errors.
- **`resolve_unique_slug()`** for soft-deletable entities and a **`now()`**
  timestamp helper in `nest-rs-seaorm`.
- **Actor identity on the request span** — denials are attributable
  without per-site threading.
- **Per-job spans and start/ok/fail events** in the Redis queue
  consumer.
- **`#[non_exhaustive]` on the eight public error enums**, so a new
  variant is no longer a breaking change, and compiler-enforced
  unsafe-freedom via `[workspace.lints] unsafe_code = "forbid"`, opted
  into workspace-wide with three documented exceptions.
- **Bounded WebSocket connection lifetime** (`WsConfig`, default 4h)
  and an OpenAPI enable toggle.
- **`nest-rs-testing` auto-loads the project `.env`** for e2e, so every
  boot sees the same URLs the app does — no duplicated test env file.
- `nestrs run db down [N]` reverts N migrations (default one step).
- `nestrs new` ships a `compose.yml` in the workspace scaffold.

### Changed

- **Minimum supported Rust is now 1.96** (was 1.95), pinned explicitly
  in `rust-toolchain.toml` and the workspace `rust-version`.
- **`nest-rs-macros` is renamed `nest-rs-core-macros`.** Apps consuming
  the framework through the `nest-rs` umbrella are unaffected; a direct
  dependency on the old name must be repointed.
- **`async-graphql` is pinned to `=7.2.1`** (exact, not caret): the
  resolver codegen spells out a public-but-internal registry literal
  that a minor bump can silently change. Guarded by a compile-time
  canary and an SDL snapshot test; the bump procedure lives in the
  crate docs.
- **`ConfigService::var` is renamed `var_name`** — it returns the
  variable's name, not its value, and shadowed the meaning of
  `std::env::var`.
- **`nest-rs-config` no longer mutates the process environment** on the
  live path — it reads an in-crate `.env` map, with the real
  environment winning.
- **Transport dependencies are feature-gated** (interceptors, filters,
  exception-filters, guards) so an HTTP-only app skips the GraphQL and
  WebSocket stacks.
- **Access and create authorization are decided in SQL.**
  `CrudService::access` re-checks the primary key against
  `condition_for(action)` in the database instead of an in-memory
  `Ability::can` — one source of truth with the list filter, and what
  makes relational rules enforceable on the by-id and create paths.
- **GraphQL posture is mandatory and visible.** Every operation declares
  `#[authorize(Action, Entity)]` (class gate + automatic response
  masking) or `#[public]`; an operation without a posture does not
  compile, and an `Authorized<E>` parameter is not accepted as a
  standalone posture.
- **Transfer objects are named by the boundary they cross** — REST
  `Dto`, queue `Command` / `Event`, GraphQL `Input`; entity-derived
  CRUD forms stay bare (`CreateUser`), with file-role placement to
  match.
- **Framework and product split into two Cargo workspaces** (root
  `crates/nest-rs-*` vs `demo/`), the demo consuming the framework by
  relative path.

### Fixed

- **Security: a pre-release audit pass across the framework.** All authz
  denials log at `warn`; a throttler brute-force bypass is closed
  (per-bucket window + route-scoped key); JWT `aud`/`iss` are enforced;
  a failed predicate fail-closes to `Deny` instead of panicking per
  request; OAuth state compares in constant time; submitted values are
  stripped from validation-error responses; masked responses are
  retained by a static expose set.
- **Login separates store outages from credential mismatches.** Every
  `DbErr` on the login path used to map to an invalid-credentials 401,
  hiding outages and locking out returning OAuth users. Store failures
  now surface as `AuthError::Unavailable` (500, logged at `error`),
  kept distinct from the opaque credential rejection.
- Boot fails with a named error on a duplicate controller prefix
  (previously a panic).
- Lifecycle hooks whose provider is unreachable are surfaced at boot
  instead of silently never running.
- `#[crud]` GraphQL operation names derive from the snake_case entity
  name.
- `#[public]` is rejected on WS message handlers; OAuth login input
  hardened.

### Documentation

- Content overhaul: a linear onboarding journey, a request-lifecycle
  page, corrected decorator docs with macro expansion sketches, and a
  new Entities reference page.
- Shipped `STYLE.md`, page templates, and a docs lint gate.

## [0.2.0] - 2026-06-10

### Added

- **CLI generators (`nest-rs-cli`).** New scaffolding binary with
  `nestrs g feature/resource/<transport>` — transactional scaffold core that
  generates files and auto-wires modules, with context detection.
- **`nestrs run` task front door.** Single entry point that forwards to `just`
  recipes, with first-run toolchain bootstrap (installs `just`, `bacon`,
  `cargo-nextest`, binstall-preferred; opt out via `--no-bootstrap` /
  `NESTRS_NO_BOOTSTRAP`).
- **Publish suite.** Exemplar workspace with org-scoped posts spanning REST,
  GraphQL, WebSockets, queue, and MCP apps.

### Changed

- **Unified layer pool.** Guards, pipes, interceptors, filters, and
  exception-filters now resolve through a single deduplicated pool per family
  (execute exactly once per request; broadest scope wins).
- **Apps renamed** and **service-naming conventions** tightened across the
  workspace (`svc` / `<name>_svc` injection naming).

### Fixed

- **Security: hardened authn/authz, transports, the data layer, and the CLI**
  against several edge cases.
- **Security: fail closed on unwired MCP** and **enforce a minimum HS256 secret
  length** at boot.
- Access-log `duration_ms` now rounded to microsecond precision.

### Documentation

- Added the Lifecycle fundamentals page and a dedicated packages page.
- Routed all task examples through `nestrs run`.
- Refined the splash hero / landing page (mobile layout, hello code-tabs demo,
  access-log terminal lines) and slimmed the README toward contributors,
  pointing users to nestrs.dev.

## [0.1.0] - 2026-06-08

Initial public release of the nestrs framework — an opinionated Rust framework
where the developer writes business logic and the framework carries the
cross-cutting concerns (authn, authz, row-level filtering, transactions, edge
validation, discovery, lifecycle).

### Added

- **Composition & DI.** Type-id container with `#[inject]` fields, `#[module]`
  composition, four-phase `App::builder().build()`, singleton/request/transient
  scopes, and a compile-time + boot-time access graph.
- **Request layers.** Guards, pipes, interceptors, filters, and exception
  filters with symmetric scopes (global / controller / handler) and TypeId
  dedup.
- **Transports.** HTTP (`nest-rs-http`), GraphQL (`nest-rs-graphql`),
  WebSockets (`nest-rs-ws`), queue (`nest-rs-queue` + `nest-rs-redis`),
  scheduler (`nest-rs-schedule`), MCP, and OpenAPI (`nest-rs-openapi`).
- **Authn / authz.** `nest-rs-authn` (strategies, `AuthGuard`, `JwtService`)
  and `nest-rs-authz` (abilities, ability guards, response masking) with
  bridges per transport.
- **Data layer.** `nest-rs-seaorm` with transparent ability-scoped `Repo`,
  ambient executor/transaction `task_local!`s, route-model binding, and
  auto-resolved GraphQL relations from `#[expose]`.
- **Supporting crates.** Pipes, events, health, throttler, config,
  opentelemetry, and the `nest-rs` umbrella crate (`use nest_rs::prelude::*`).
- **`nest-rs-*` naming alignment** across directories, packages, and imports;
  framework-owned error types.
- Rust 1.95 / edition 2024; tag-based release CI with the `mold` linker on
  Linux.

[1.0.0]: https://github.com/YV17labs/NestRS/compare/v0.5.0...v1.0.0
[0.5.0]: https://github.com/YV17labs/NestRS/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/YV17labs/NestRS/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/YV17labs/NestRS/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/YV17labs/NestRS/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/YV17labs/NestRS/releases/tag/v0.1.0

# Roadmap

NestRS is in **alpha** — the foundations are in place and the API still shifts.
This is a *direction, not a dated commitment*; priorities move with what the
community needs.

Want to shape it? Open a
[Discussion](https://github.com/NestRS/NestRS/discussions) or pick up a
[`good first issue`](https://github.com/NestRS/NestRS/labels/good%20first%20issue).
The authoritative record of *what was decided and why* is
[CLAUDE.md](CLAUDE.md); this file tracks *what's next*.

## Recently shipped

- **Transparent security, conversion & transactions** — the mission's hardest
  promise, now built on one primitive: a request-scoped *data context* (two
  `tokio` task-locals carrying the current DB executor and the caller's `Ability`).
  A service queries through `Repo<E>` (`nestrs-orm`) — every call runs against the
  ambient executor and every read is filtered by the caller's `condition_for`, so
  **row-level security and transactions need no hand-written code**. An
  auto-installed `DbContext` interceptor (just import `DatabaseModule`) makes the
  executor ambient and wraps each mutating request in a transaction (commit on
  2xx/3xx, roll back otherwise). The `Bind<E, A>` extractor is **route-model
  binding** (path id → loaded, authorized entity, with `400`/`404`/`403`
  short-circuits), and `Scope<E, A>` yields the row filter as an explicit argument.
  `apps/api` is the exemplar — `UsersController::get` is a single `Bind` parameter
  and `create` is transactional with no annotation. See *The data layer makes
  security and transactions transparent* in [CLAUDE.md](CLAUDE.md).
- **Rate limiting** — `nestrs-throttler`: a per-route `ThrottlerGuard`
  (`#[use_guards(ThrottlerGuard)]`) reading an optional `#[meta(Throttle::...)]`
  override (the `@nestjs/throttler` `ThrottlerGuard` + `@Throttle` analog), backed
  by an in-memory fixed-window counter keyed by client IP; over-limit requests get
  `429` + `Retry-After`. `ThrottlerModule::for_root` sets the default. `apps/auth`
  rate-limits its `POST /token` endpoint. A Redis-backed store for multi-process
  deployments is the remaining piece.
- **Authentication** — `nestrs-auth`: a `JwtService` (sign/verify, the `@nestjs/jwt`
  analog) made injectable everywhere by `AuthModule::for_root`; a pluggable
  `Strategy` trait + the request-scoped `AuthGuard<S>` (the `AuthGuard('jwt')`
  analog) that establishes the caller and attaches it for `nestrs-authz` to
  authorize; and an `OAuth2Client` for the Authorization Code flow (PKCE, with the
  CSRF/PKCE state carried in a `JwtService`-signed cookie — no server-side
  session). One `Strategy` serves both bearer tokens and the redirect-based OAuth
  handshake. `JwtService` also does asymmetric EdDSA (sign vs verify-only), which
  splits issuance from verification across apps: `apps/auth` issues tokens (OAuth2
  `POST /token` + the redirect flow, signing with the private key) and `apps/api`
  is a pure resource server that only verifies them with the public key.
- **Cron expressions** — `#[cron_job(cron = "0 */5 * * * *")]` (the `@Cron` analog),
  with `CronExpression` presets, an optional `tz` (IANA name, default UTC), and a
  one-shot `after = "10s"` (the `@Timeout` analog) joining the existing interval
  `every` (the `@Interval` analog). Parsed by `croner` over `chrono`; a literal is
  validated at compile time, a preset and any timezone at boot.
- **Request-scoped providers** — `#[injectable(scope = request)]` builds a fresh,
  per-request-cached instance (the `Scope.REQUEST` analog), resolved over HTTP via
  the `Scoped<T>` extractor.
- **Resource pagination** — `#[expose(paginate)]` emits a `<Name>Page` envelope and
  the shared `PageArgs` input, serving GraphQL and OpenAPI alike.
- **URI API versioning & per-route filters** — `#[controller(version = "1")]`
  mounts a controller under `/v1` (one source of truth for the served path, the
  boot log, and the OpenAPI document); `#[use_filters(...)]` binds exception
  filters to a single route (the `@UseFilters` analog).
- **`nestrs-testing` + an e2e per app** — the in-process harness boots the real
  DI graph and drives its surfaces inside `cargo test`, with provider overrides
  for mocking (the `Test.createTestingModule` analog). It now also boots
  **headless** for non-HTTP transports (the queue worker) and ships fixtures: an
  ephemeral, migrated Postgres database and the telemetry boot guard. Every
  example app ships an end-to-end test.
- **Richer boot diagnostics** — the DI graph names the offending provider and the
  missing dependency, distinguishes a missing provider from a dependency cycle,
  and rejects a non-`Arc` `#[inject]` at compile time.
- **Per-handler metadata + `Reflector`** — `#[meta(...)]` on a handler, read back
  by a guard via `nestrs_http::Reflector` (the `@Roles` / `@SetMetadata` analog).
- **GraphQL authorization** — `nestrs-authz-graphql` gates resolvers with the
  request-scoped `Ability`, carried into the GraphQL context by a per-request
  bridge in `nestrs-graphql`.
- **CORS** — `HttpTransport::cors(...)` (the `app.enableCors` analog).
- **Optional dependencies** — `#[inject] Option<Arc<T>>` (the `@Optional` analog),
  resolved leniently and independent of `providers = [...]` order.
- **Config validation** — `nestrs_config::load_validated` runs a config type's
  `validator` rules at startup, so a malformed environment fails fast.
- **Events** — `nestrs-events`: a typed in-process event bus and an
  `#[event_handler]` decorator (the `@nestjs/event-emitter` analog), wired at
  application bootstrap from the assembled container.
- **Telemetry fail-fast** — importing `TelemetryModule` without `Telemetry::init`
  now fails at boot rather than dropping traces and metrics silently.

## Now — stabilising the alpha

- Settle the public API of the core crates so early adopters stop chasing
  breaking changes.
- **Published benchmarks** — reproducible throughput and memory numbers now ship
  in the README (the `app` example versus an equivalent NestJS service); cold-start
  numbers are still to follow.
- Fill in crate-level docs and grow the `apps/` examples.

## Next — the documented gaps

These are known, deliberate omissions called out in the code today:

- **OpenAPI** — query-parameter schemas, real path-parameter *types* (emitted as
  `string` for now), security schemes, and a committed `openapi.json` snapshot
  written on boot (mirroring how the GraphQL SDL is committed).
- **Dependency-injection scopes** — request scope already ships (see above); what
  remains is a `transient` scope (fresh per resolution), request-scoped →
  request-scoped dependencies (the model is one level deep over singletons today),
  and bridging the scope into the GraphQL and MCP request paths (which carry
  per-request state through their own context / DataLoaders for now).
- **`nestrs-resource`** — pagination already ships (see above). Relations are
  *not* auto-generated by design — a related field is a hand-written `#[field]`
  resolver backed by a `#[dataloader]` on the data layer (the framework's batched,
  N+1-free pattern), since the loader belongs to the service, not the entity. An
  enum column passes through as long as it derives the surface traits
  (async-graphql `Enum` + `schemars::JsonSchema`); a first-class `#[expose]` enum
  mode is the remaining gap.
- **API versioning strategies** — URI versioning already ships (see above);
  header- and media-type-based selection (NestJS's other `VersioningType`s, which
  need request-time dispatch) are not yet built.

## Next — feature parity with NestJS

These are NestJS building blocks an app still has to hand-roll. Listed because
they are *load-bearing for real use*, not for completeness — each maps to a known
NestJS name and earns its place. (Authentication and rate limiting, formerly the
standout gaps here, now ship — see *Recently shipped*.) The verdict on what is
**not** worth reproducing is in *Not on the roadmap* below.

- **Redis-backed rate-limit store** — `nestrs-throttler` ships with an in-memory
  fixed-window counter; a Redis store would share limits across processes (the
  `@nest-lab/throttler-storage-redis` analog), reusing the queue's connection
  pattern. The guard would take a storage trait object then.
- **Caching** — a `CacheModule` + a response-caching interceptor + an injectable
  `Cache` (the `@nestjs/cache-manager` analog), memory- or Redis-backed. Common,
  though an app survives without it.
- **File upload & streaming responses** — a multipart extractor for uploads and a
  `StreamableFile` response (the `FileInterceptor` / `StreamableFile` analog) for
  large or generated payloads.
- **OpenAPI completeness** — already under *the documented gaps* above, repeated
  here because it is an *incomplete shipped feature*, not a future one: the emitted
  document omits query parameters entirely and types every path parameter as
  `string`. Documenting security schemes becomes the highest-value fix once auth
  lands.

## Next — extending the transparent data layer

The transparent data context (see *Recently shipped*) covers HTTP today. What
remains builds on the same primitive:

- **A reusable GraphQL auth bridge + scoped dataloaders** — `apps/api` already
  scopes GraphQL the same as REST: a `GraphqlAuth` interceptor runs the guard chain
  on `/graphql` and installs the ambient ability, so resolver `Repo` reads filter by
  org and `authorize::<A, S>(ctx)` gates each resolver. Two gaps remain. First, that
  bridge is hand-wired per app; a generic `GraphqlAuth<S, F>` (parameterized by the
  app's `Strategy` + `AbilityFactory`, like `AuthGuard<S>`) would move it into the
  framework. Second, a `#[dataloader]` batch runs off the request task, so the
  ambient ability never reaches it — a field-relation loader read is unscoped and
  must be confined by hand today (e.g. filtering to the parent's org). Threading the
  request ability into the per-request loaders would close that.
- **Ability-scoped writes** — `Repo` auto-scopes *reads*; an update/delete could
  likewise gate its `WHERE` on `condition_for(Update/Delete)`, so a caller cannot
  mutate a row outside its scope even by id.
- **A request executor for non-HTTP transports** — the `DbContext` interceptor binds
  the executor to an HTTP request; a queue job or cron tick has no ambient executor,
  so those paths still inject `Arc<DatabaseConnection>`. A transport-agnostic
  installer would extend `Repo` (and per-job transactions) to the worker surfaces.

## Next — project & release infrastructure

None of this exists yet; it is what turns the workspace into a project others can
build on and contribute to. The repo stays a **single monorepo** (the model every
multi-crate Rust framework uses — `tokio`, `bevy`, `axum`): one atomic commit can
span a crate, its `*-macros` companion, and an example app, which a repo-per-crate
split would make impossible.

- **Continuous integration** — one workflow on every PR that gates merges:
  `fmt --check`, `clippy -D warnings`, `build`, and `test --workspace`. The e2e
  tests exercise live Postgres and Redis, so CI provisions both as service
  containers. It publishes nothing — its only artifact is a green/red signal.
- **Release automation** — versions move in **lockstep** (one number for the whole
  workspace, centralised in `[workspace.package]` so a single line bumps every
  crate) while the alpha API churns; independent per-crate versioning waits until
  crates stabilise at different rates. Publishing to crates.io is automated — a
  release PR bumps versions and changelogs, then publishes each crate in dependency
  order; nothing is built or uploaded by hand. The `apps/` stay `publish = false`.
- **A `nestrs` facade crate** — re-exports the building blocks behind one
  dependency and one feature set, so an app adds `nestrs` rather than wiring the
  internal crates by name (the way `tokio` and `bevy` front their workspaces). It
  is also the single version an app pins.
- **A scaffolding CLI** — `nestrs new <app>` generates a working starter, and
  generators (`nestrs g controller`, `... entity`, `... resource`) emit the
  declarative boilerplate from the same macros apps use (the `nest` / `nest g`
  analog). It ships as another workspace crate; the starter is a template the CLI
  instantiates, and a generated project is the user's own repo depending on the
  published crates.
- **A GitHub organisation** — one canonical home and repository URL (the
  `Cargo.toml` `repository` and the docs currently disagree on the owner), with a
  single primary repo — an org is for branding and collaborators, not a reason to
  split the crates apart.

## Later — exploring

- **Real-time surface** — WebSocket gateways (the `@WebSocketGateway` /
  `@SubscribeMessage` analog), Server-Sent Events (`@Sse`), and GraphQL
  subscriptions (`EmptySubscription` today). One transport effort; built when an
  example app genuinely needs it, not speculatively.
- GraphQL **federation**, and the dedicated schema tooling it would reintroduce.
- More transports and surfaces as the discovery model proves out.

## Not on the roadmap

By design — see the *Hard "no" list* in [CLAUDE.md](CLAUDE.md):

- No external dependency-injection library — the container is ours.
- No splitting the workspace into microservices "for scalability".
- No backwards-compatibility shims while the API is pre-1.0.
- **No `ClassSerializerInterceptor` / `@Exclude` / `@Expose`** — serde already owns
  serialization (`#[serde(skip)]`, or a dedicated response DTO); a per-request
  "groups" interceptor is not worth reproducing.
- **No `HttpModule` / `HttpService`** — inject a configured `reqwest::Client`; an
  axios-style wrapper would be pure ceremony.
- **No NestJS `Logger` service** — `tracing` is the idiomatic, structured, superior
  answer, and is already the project's logging layer.

# Roadmap

NestRS is in **alpha** — the foundations are in place and the API still shifts.
This is a *direction, not a dated commitment*; priorities move with what the
community needs. The `Next —` sections below are ordered by **integration
priority** — finishing real-time, then correctness and parity work; `Later`
holds what is explicitly deferred.

Want to shape it? Open a
[Discussion](https://github.com/NestRS/NestRS/discussions) or pick up a
[`good first issue`](https://github.com/NestRS/NestRS/labels/good%20first%20issue).
The authoritative record of *what was decided and why* is
[CLAUDE.md](CLAUDE.md); this file tracks *what's next*.

## Now — stabilising the alpha

- Settle the public API of the core crates so early adopters stop chasing
  breaking changes.
- **Cold-start benchmark** — throughput and memory numbers already ship in the
  README (the `app` example versus an equivalent NestJS service); the cold-start
  figure is the remaining one to publish.
- Fill in crate-level docs and grow the `apps/` examples.

## Done — real-time: the WebSocket gateway

The gateway ships complete: request/response message handling, a connection
registry, broadcast, rooms and per-gateway namespacing, discovered and
self-mounted on the HTTP transport, sharing controller DI, connection-level
*and* per-message guards, `on_connect` / `on_disconnect` lifecycle hooks, and now
the ambient data context — the plumbing Server-Sent Events and GraphQL
subscriptions will also reuse:

- **Server→client broadcast, a connection registry & per-gateway namespacing** —
  *shipped*. `WsServer<N>` (the `@WebSocketServer` analog, the `Global` namespace
  provided by `WsModule`) tracks live connections and rooms; a service injects
  `Arc<WsServer>` to push in reaction to a domain event, and a handler reaches it
  through a `&WsClient` parameter (the `@ConnectedSocket` analog). A
  `#[gateway(namespace = MyNs)]` mounts against its own `WsServer<MyNs>` — a
  separate registry the macro self-provides — so two gateways isolate without
  sharing one; the handler surface stays namespace-free because `WsClient` carries
  the registry type-erased as `Registry`. `apps/chat` proves the isolation over a
  real socket.
- **Per-message guards & lifecycle hooks** — *shipped*. A `#[use_guards]` beside
  a `#[subscribe_message]` binds per-message `MessageGuard`s (its context is the
  message, not the upgrade request — an `Err` short-circuits to an error frame
  before the handler runs), complementing the connection-level guards on the
  gateway struct; and an `#[on_connect]` / `#[on_disconnect]` method on the
  `#[messages]` impl block is the `OnGatewayConnection` / `OnGatewayDisconnect`
  analog. `apps/chat` exercises both over a real socket.
- **An ambient data context in the socket task** — *shipped*. The connection loop
  runs *after* the upgrade request completes (the global `DbContext` interceptor's
  executor scope and the authz ability both unwind with that request), so neither
  task-local reached a message handler — the same constraint a `#[dataloader]`
  batch has. `nestrs-ws` now exposes an orm/authz-agnostic per-connection hook,
  the `SocketContext` seam (mirroring GraphQL's `OperationGuard`): it captures
  opaque per-connection state from the post-guard upgrade request, then wraps each
  dispatch. The `nestrs-authz-ws` bridge implements it by re-installing the
  executor (`with_executor`, pool) and the caller's ability (`with_ability`,
  captured from the connection guards) around the handler future — so a gateway
  handler uses `Repo` like a controller, row-level filtering included. `apps/api`
  is the exemplar (a `UsersGateway` whose `users.list` scopes to the caller's org)
  with a DB-backed, authenticated real-socket e2e. The executor binds the
  connection **pool**, so a message runs without a per-message transaction (a
  WebSocket message has no safe/mutating HTTP method to classify) — the one piece
  deliberately deferred.

## Next — extending the transparent data layer

The transparent data context covers HTTP, GraphQL (resolvers *and* dataloaders),
and WebSocket today. What remains builds on the same primitive:

- **Scoped dataloaders** — *shipped*. A `#[dataloader]` batch runs *off* the
  request task (async-graphql spawns it to collapse concurrent `load_one`s into one
  query), so the ambient ability never used to reach it — a field-relation loader
  read was **unscoped** and had to be confined by hand. `nestrs-graphql` now exposes
  the `BatchContext` seam (the spawner each per-request `DataLoader` is built with,
  resolved via `get_dyn`), and `nestrs-authz-graphql`'s `LoaderScope` implements it:
  built inside the operation's `with_ability` scope, it snapshots the live ability
  and re-establishes it (plus a pool executor) around every batch future. So a
  loader's `Repo` reads scope to the caller transparently, with no hand-written
  filter — closing the last place row-level security leaned on developer discipline.
  Bound by listing `LoaderScope as dyn BatchContext`; `apps/api` is the exemplar
  (a DB-backed cross-org `namesakes` e2e proves the other org's rows never reach the
  batch).
- **Ability-scoped writes** — *shipped*. `Repo` auto-scopes *reads*; now
  [`Repo::update`]/[`Repo::delete`] gate their `WHERE` on
  `condition_for(Update/Delete)` on top of the primary key, so a caller cannot
  mutate or delete a row outside its scope even by id — the scope-excluded write
  touches nothing and surfaces as `RecordNotUpdated` / a zero-row result (both
  logged at `warn`). `CrudService::update`/`delete` route through them, so every
  surface (REST, GraphQL, gateways) inherits the gate transparently; it is defense
  in depth behind the `access` class gate, catching any path that reaches a write
  with a row loaded out-of-scope. `apps/api` proves it with a direct-`Repo`
  cross-org e2e (the row survives both attempts).
- **A request executor for non-HTTP transports** — *shipped*. The `DbContext`
  interceptor binds the executor to an HTTP request and `SocketContext` to a
  WebSocket message; now `nestrs-core`'s orm-agnostic `JobContext` seam carries it to
  the worker surfaces too. Both worker transports resolve an optional implementor
  from the container (`get_dyn`) and wrap each job through it; `nestrs-orm`'s
  `WorkerDbContext` implements it to install a **pool** executor, and `DatabaseModule`
  auto-binds it (like `DbContext` for HTTP) — so importing the database module gives a
  `#[cron_job]`/`#[processor]` an ambient `Repo` with no `Arc<DatabaseConnection>`
  injected. With nothing bound a job runs bare (the default). The remaining piece is
  **per-job transactions** (a worker job has no safe/mutating method to classify, so
  it runs on the pool like a WebSocket message — deliberately deferred).

## Next — hardening the guarantees

The framework's promises — transparent security, a DI graph checked at boot,
declarative wiring — hold today but lean on developer discipline at a few seams.
Closing these is what makes the guarantees *airtight*, which is the real edge over
a framework that only **documents** the same concerns.

- **Transaction-escape safety** — *shipped*. The auto-transaction commits by
  reclaiming the request's `Arc<DatabaseTransaction>` with `Arc::try_unwrap`; if a
  handler leaks the executor to a spawned task, the unwrap fails and the request can
  no longer commit. Before, that **rolled back on a 2xx** — a success response with
  nothing persisted (logged, but not surfaced to the caller). Now the `DbContext`
  interceptor detects the escape and fails *loudly*: a 2xx/3xx answer is replaced
  with a `500` so a lost write is never reported as committed (a response that had
  already failed keeps its status — its rollback matches its intent). An in-process
  test drives a handler that leaks the executor into a detached task and asserts the
  otherwise-successful response surfaces as a `500`, not a false `2xx`.
- **A total access contract** — *attribute-referenced layers: shipped; resolver
  injection: remaining.* The boot-time access graph governs `#[inject]`
  dependencies; three declarative seams used to sit outside it. The
  `#[use_guards]` / `#[use_filters]` / `#[use_interceptors]` references (controller-
  and per-route scope, plus the WebSocket gateway's connection- and per-message
  guards) now fold their `TypeId`s into the consumer's `Discoverable::injected`, so
  a layer registered in a module the consumer does not import fails the boot with
  the same named `AccessGraphError` an out-of-reach `#[inject]` gets — closing a
  *silent cross-module encapsulation breach* (the flat container resolved the layer
  regardless of imports), not merely a panic. `nestrs-testing`'s `access_contract`
  e2e proves both scopes fail the boot and a correctly-imported one passes. The
  remaining seam is **`#[resolver]` injection**: a resolver self-composes through
  the GraphQL registry and belongs to *no* module, so there is no import closure to
  check it against — bringing it under the contract needs a resolver-membership
  design decision, not just plumbing, and is deferred behind a design note.
- **Insulate the GraphQL schema composition** — the self-composing schema reads
  async-graphql's public-but-internal `registry` API. It is guarded by tests, but a
  thin adapter (one place that breaks, behind a pinned-version compile guard) would
  keep an async-graphql bump from rippling through the crate.
- **Keyed / multi-instance providers** — the flat container keys by type, so a
  second instance of a type (two `OAuth2Client`s, for GitHub *and* Google) needs a
  hand-written newtype today. A keyed registration (`provide_keyed`) would let one
  type back several named instances without the ceremony.
- **Compile-time guardrails for the stringly-typed seams** — a queue name is a
  string shared between the producer and its `#[processor]`, and a dataloader's
  generated loader type (`UsersServiceByName`) is found by naming convention; a typo
  surfaces at runtime or as a cryptic type error. Typed queue handles and a clearer
  loader-type surface would move both to compile time (a guard-order lint — authn
  before authz — is the same class of guardrail).

## Next — the documented gaps

These are known, deliberate omissions called out in the code today:

- **OpenAPI** — query-parameter schemas, real path-parameter *types* (emitted as
  `string` for now), security schemes, and a committed `openapi.json` snapshot
  written on boot (mirroring how the GraphQL SDL is committed).
- **Dependency-injection scopes** — request scope already ships; what
  remains is a `transient` scope (fresh per resolution), request-scoped →
  request-scoped dependencies (the model is one level deep over singletons today),
  and bridging the scope into the GraphQL and MCP request paths (which carry
  per-request state through their own context / DataLoaders for now).
- **`nestrs-resource`** — pagination already ships. Relations are
  *not* auto-generated by design — a related field is a hand-written `#[field]`
  resolver backed by a `#[dataloader]` on the data layer (the framework's batched,
  N+1-free pattern), since the loader belongs to the service, not the entity. An
  enum column passes through as long as it derives the surface traits
  (async-graphql `Enum` + `schemars::JsonSchema`); a first-class `#[expose]` enum
  mode is the remaining gap.
- **API versioning strategies** — URI versioning already ships;
  header- and media-type-based selection (NestJS's other `VersioningType`s, which
  need request-time dispatch) are not yet built.
- **TLS certificate hot-reload** — `HttpTransport::tls` loads the certificate
  once at boot; rotating it on renewal needs a restart today. Watching the PEM
  source and swapping the `rustls` config live would close it.

## Next — feature parity with NestJS

These are NestJS building blocks an app still has to hand-roll. Listed because
they are *load-bearing for real use*, not for completeness — each maps to a known
NestJS name and earns its place. (Authentication and rate limiting, formerly the
standout gaps here, now ship.) The verdict on what is
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

Not current priorities — WebSocket, the one wanted transport, now ships; these follow only when an example app genuinely needs them.

- **Server-Sent Events & GraphQL subscriptions** — `@Sse` and a real subscription
  root (`EmptySubscription` today); both reuse the WebSocket gateway's
  per-connection plumbing once the broadcast/registry piece above lands.
- **gRPC** and other request/response transports, as the discovery model proves out.
- GraphQL **federation**, and the dedicated schema tooling it would reintroduce.

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

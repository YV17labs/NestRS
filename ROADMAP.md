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

## Next — real-time: completing the WebSocket gateway

The gateway now ships with server→client push — request/response message
handling, a connection registry, broadcast and rooms, discovered and
self-mounted on the HTTP transport, sharing controller DI, connection-level
*and* per-message guards, and `on_connect` / `on_disconnect` lifecycle hooks.
What remains builds on that base, and is the plumbing Server-Sent Events and
GraphQL subscriptions will reuse:

- **Server→client broadcast & a connection registry** — *shipped*. `WsServer`
  (the `@WebSocketServer` analog, provided by `WsModule`) tracks live connections
  and rooms; a service injects `Arc<WsServer>` to push in reaction to a domain
  event, and a handler reaches it through a `&WsClient` parameter (the
  `@ConnectedSocket` analog). The remaining gap is **per-gateway namespacing**:
  the flat container keys `WsServer` by type, so every gateway shares one registry
  (rooms are the targeting tool); a second registry needs a newtype today.
- **Per-message guards & lifecycle hooks** — *shipped*. A `#[use_guards]` beside
  a `#[subscribe_message]` binds per-message `MessageGuard`s (its context is the
  message, not the upgrade request — an `Err` short-circuits to an error frame
  before the handler runs), complementing the connection-level guards on the
  gateway struct; and an `#[on_connect]` / `#[on_disconnect]` method on the
  `#[messages]` impl block is the `OnGatewayConnection` / `OnGatewayDisconnect`
  analog. `apps/chat` exercises both over a real socket.
- **An ambient data context in the socket task** — the connection loop runs
  *after* the upgrade request completes, so the request scope, ORM executor and
  authz ability task-locals don't reach a handler (the same constraint a
  `#[dataloader]` batch has). A transport-agnostic installer (see *extending the
  transparent data layer*) would let a gateway handler use `Repo` like a
  controller.

## Next — extending the transparent data layer

The transparent data context covers HTTP today. What
remains builds on the same primitive:

- **Scoped dataloaders** — GraphQL is already authorized the same as REST (the
  generic `GraphqlAbilityBridge<A, G>` runs the guard chain on `/graphql` and
  installs the ambient ability), so resolver `Repo` reads
  filter by org. The remaining gap is the loaders: a `#[dataloader]` batch runs
  *off* the request task, so the ambient ability never reaches it — a field-relation
  loader read is **unscoped** and must be confined by hand today (e.g. filtering to
  the parent's org). Threading the request ability into the per-request loaders
  would close it — the one place row-level security still leans on developer
  discipline.
- **Ability-scoped writes** — `Repo` auto-scopes *reads*; an update/delete could
  likewise gate its `WHERE` on `condition_for(Update/Delete)`, so a caller cannot
  mutate a row outside its scope even by id.
- **A request executor for non-HTTP transports** — the `DbContext` interceptor binds
  the executor to an HTTP request; a queue job or cron tick has no ambient executor,
  so those paths still inject `Arc<DatabaseConnection>`. A transport-agnostic
  installer would extend `Repo` (and per-job transactions) to the worker surfaces.

## Next — hardening the guarantees

The framework's promises — transparent security, a DI graph checked at boot,
declarative wiring — hold today but lean on developer discipline at a few seams.
Closing these is what makes the guarantees *airtight*, which is the real edge over
a framework that only **documents** the same concerns.

- **Transaction-escape safety** — the auto-transaction commits by reclaiming the
  request's `Arc<DatabaseTransaction>` with `Arc::try_unwrap`; if a handler leaks
  the executor to a spawned task, the unwrap fails and the request **rolls back on
  a 2xx** — a success response with nothing persisted (logged, but not surfaced to
  the caller). Detect the escape explicitly, so the failure mode is a loud error,
  not silent data loss.
- **A total access contract** — the boot-time access graph governs `#[inject]`
  dependencies, but three declarative seams sit outside it: a `#[use_guards]` /
  `#[use_filters]` / `#[use_interceptors]` reference resolves from the container at
  mount (an unregistered one panics at boot instead of raising the named
  `AccessGraphError` everything else gets), and `#[resolver]` injection is unchecked.
  Bringing the attribute-referenced layers and the resolver layer under the same
  check turns the last "panic / discipline" cases into the same boot diagnostic.
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

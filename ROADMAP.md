# Roadmap

NestRS is in **alpha** — the foundations are in place and the API still shifts.
This is a *direction, not a dated commitment*; priorities move with what the
community needs. The sections below are ordered by **priority** — correctness and
parity work first; `Later` holds what is explicitly deferred.

Want to shape it? Open a
[Discussion](https://github.com/NestRS/NestRS/discussions) or pick up a
[`good first issue`](https://github.com/NestRS/NestRS/labels/good%20first%20issue).
The authoritative record of *what was decided and why* is
[CLAUDE.md](CLAUDE.md); this file tracks *what's next*.

## Now — stabilising the alpha

- Settle the public API of the core crates so early adopters stop chasing
  breaking changes.
- **Cold-start benchmark** — publish the cold-start figure alongside the
  throughput and memory numbers already in the README.
- Fill in crate-level docs and grow the `apps/` examples.

## Next — hardening the guarantees

The framework's promises — transparent security, a DI graph checked at boot,
declarative wiring — hold today but lean on developer discipline at a few seams.
Closing these is what makes the guarantees *airtight*, the real edge over a
framework that only **documents** the same concerns.

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

## Next — completing shipped features

Known, deliberate gaps in features that already ship:

- **OpenAPI completeness** — the emitted document omits query parameters entirely,
  types every path parameter as `string`, and documents no security schemes;
  a committed `openapi.json` snapshot written on boot (mirroring the GraphQL SDL)
  is also missing.
- **Dependency-injection scopes** — what remains beyond request scope is a
  `transient` scope (fresh per resolution), request-scoped → request-scoped
  dependencies (the model is one level deep over singletons today), and bridging
  the scope into the GraphQL and MCP request paths (which carry per-request state
  through their own context / DataLoaders for now).
- **`nest-rs-resource`** — a first-class `#[expose]` enum mode (an enum column
  already passes through if it derives the surface traits), HasMany pagination
  via `Connection<T>` (the auto-emitted resolver returns a raw `Vec<T>` today —
  fine at small N, a DoS waiting at large N), and a `via = "..."` override for
  HasMany so non-conventional FK columns work without falling back to a manual
  `#[field_resolver]`.
- **API versioning strategies** — header- and media-type-based selection
  (which need request-time dispatch).
- **TLS certificate hot-reload** — `HttpTransport::tls` loads the certificate once
  at boot; rotating it on renewal needs a restart today. Watching the PEM source and
  swapping the `rustls` config live would close it.

## Next — common building blocks

Common server building blocks an app still has to hand-roll. Listed because they are
*load-bearing for real use*, not for completeness — each a well-understood primitive.
The verdict on what is **not** worth reproducing is in *Not on the roadmap*.

- **Redis-backed rate-limit store** — `nest-rs-throttler` ships with an in-memory
  fixed-window counter; a Redis store would share limits across processes,
  reusing the queue's connection pattern. The guard would take a storage trait
  object then.
- **Caching** — a `CacheModule` + a response-caching interceptor + an injectable
  `Cache` provider, memory- or Redis-backed.
- **File upload & streaming responses** — a multipart extractor for uploads and a
  `StreamableFile` response for large or generated payloads.

## Next — project & release infrastructure

What turns the workspace into a project others can build on and contribute to. The
repo stays a **single monorepo** (the model every multi-crate Rust framework uses —
`tokio`, `bevy`, `axum`): one atomic commit can span a crate, its `*-macros`
companion, and an example app, which a repo-per-crate split would make impossible.

- **Grow the `docs/` site** — the Starlight skeleton under `docs/` is live
  (getting started, core concepts, and one page per surface). What remains:
  the end-to-end tutorial, the Basics → All options tier split per section,
  CI-verified code snippets, and pages aligned with the current API (authz,
  testing harness). Adoption lives or dies on this — it is a release blocker,
  not a nicety.
- **Continuous integration** — one workflow on every PR that gates merges:
  `fmt --check`, `clippy -D warnings`, `build`, and `test --workspace`. The e2e
  tests exercise live Postgres and Redis, so CI provisions both as service
  containers. It publishes nothing — its only artifact is a green/red signal.
- **Release automation** — versions move in **lockstep** (one number for the whole
  workspace, centralised in `[workspace.package]`) while the alpha API churns;
  independent per-crate versioning waits until crates stabilise at different rates.
  Publishing to crates.io is automated — a release PR bumps versions and changelogs,
  then publishes each crate in dependency order. The `apps/` stay `publish = false`.
- **A `nestrs` facade crate** — re-exports the building blocks behind one
  dependency and one feature set, so an app adds `nestrs` rather than wiring the
  internal crates by name (the way `tokio` and `bevy` front their workspaces). It
  is also the single version an app pins.
- **A scaffolding CLI** — `nestrs new <app>` generates a working starter, and
  generators (`nestrs g controller`, `... entity`, `... resource`) emit the
  declarative boilerplate from the same macros apps use. It ships as another
  workspace crate.
- **A GitHub organisation** — one canonical home and repository URL (the
  `Cargo.toml` `repository` and the docs currently disagree on the owner), with a
  single primary repo.

## Later — deferred

Not current priorities; these follow only when an example app genuinely needs them.

- **Per-job transactions** — a `#[cron_job]`/`#[processor]` runs on the connection
  **pool** (a worker job has no safe/mutating method to classify, like a WebSocket
  message), so it has no per-job transaction. Deliberately deferred.
- **Server-Sent Events & GraphQL subscriptions** — `@Sse` and a real subscription
  root (`EmptySubscription` today); both reuse the WebSocket gateway's
  per-connection plumbing.
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
- **No bundled `Logger` service** — `tracing` is the idiomatic, structured, superior
  answer, and is already the project's logging layer.

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
  throughput and memory numbers already in the README (boot time is claimed in
  prose today, but not measured in the benchmark table).

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

- **OpenAPI completeness** — the emitted document omits query and header
  parameters, types every path parameter as `string`, and documents no security
  schemes; a committed `openapi.json` snapshot written on boot (mirroring the
  GraphQL SDL) is also missing.
- **Dependency-injection scopes** — request and transient scopes ship on HTTP via
  `Scoped<T>`. What remains is request-scoped → request-scoped dependencies (the
  model is one level deep over singletons today), and bridging the scope into the
  GraphQL and MCP request paths (which carry per-request state through their own
  context / DataLoaders for now).
- **`nest-rs-resource`** — a first-class `#[expose]` enum mode (an enum column
  already passes through if it derives the surface traits), HasMany pagination
  via `Connection<T>` (the auto-emitted resolver returns a raw `Vec<T>` today —
  fine at small N, a DoS waiting at large N), and a `via = "..."` override for
  HasMany so non-conventional FK columns work without falling back to a manual
  `#[field_resolver]`.
- **API versioning strategies** — header- and media-type-based selection
  (which need request-time dispatch); URI versioning (`#[controller(version =
  "1")]`) already ships.
- **TLS certificate hot-reload** — `HttpTransport::tls` loads the certificate once
  at boot; rotating it on renewal needs a restart today. Watching the PEM source and
  swapping the `rustls` config live would close it.

## Next — common building blocks

Common server building blocks an app still has to hand-roll. Listed because they are
*load-bearing for real use*, not for completeness — each a well-understood primitive.
The verdict on what is **not** worth reproducing is in *Not on the roadmap*.

- **Redis-backed rate-limit store** — `nest-rs-throttler` ships with an in-memory
  fixed-window counter and a `ThrottlerStore` trait; a Redis implementation would
  share limits across processes, reusing the queue's connection pattern.
- **Caching** — a `CacheModule` + a response-caching interceptor + an injectable
  `Cache` provider, memory- or Redis-backed.
- **File upload & streaming responses** — a multipart extractor for uploads and a
  `StreamableFile` response for large or generated payloads.

## Shipped — project & release infrastructure

Landed with the first `0.1.0` crates.io release and the alpha docs push:

- **crates.io publishing** — every `nest-rs-*` framework crate is on
  [crates.io](https://crates.io/crates/nest-rs); `apps/` and product crates stay
  `publish = false`.
- **Release automation** — versions move in **lockstep** (one number for the whole
  workspace, centralised in `[workspace.package]`). Push a `v*.*.*` tag and
  `.github/workflows/publish.yml` runs `cargo publish --workspace` in dependency
  order. Independent per-crate versioning waits until crates stabilise at different
  rates.
- **The `nest-rs` facade crate** — one dependency and one feature set on
  crates.io (`nest-rs`); in code, `use nest_rs::prelude::*;` for the common case.
- **GitHub organisation** — canonical home at
  [github.com/NestRS/NestRS](https://github.com/NestRS/NestRS).
- **The [nestrs.dev](https://nestrs.dev) docs site** — Starlight site with getting
  started, an end-to-end tutorial, fundamentals, and one section per surface crate
  (HTTP, GraphQL, security, database, queue, events, MCP, health, OpenTelemetry,
  testing, …). Published on every push to `main` via
  `.github/workflows/docs-pages.yml`.
- **Reference apps** — six example binaries under `apps/` (`hello`, `platform-auth`,
  `platform-api`, `platform-worker`, `mcp`, `chat`), plus a multi-binary Docker
  image and a dev container with Postgres and Redis.
- **Crate-level READMEs** — every framework crate ships a minimal `README.md`
  (description + links to [nestrs.dev](https://nestrs.dev) and GitHub);
  extension-point contracts live in the docs site.

## Next — project & release infrastructure

What remains before the workspace is easy to adopt and contribute to. The repo
stays a **single monorepo** (the model every multi-crate Rust framework uses —
`tokio`, `bevy`, `axum`): one atomic commit can span a crate, its `*-macros`
companion, and an example app, which a repo-per-crate split would make impossible.

- **Polish the `docs/` site** — CI-verified code snippets, the Basics → All options
  tier split per section, and keeping pages aligned as the API shifts during alpha.
- **Continuous integration** — one workflow on every PR that gates merges:
  `fmt --check`, `clippy -D warnings`, `build`, and `test --workspace`. The e2e
  tests exercise live Postgres and Redis, so CI provisions both as service
  containers. It publishes nothing — its only artifact is a green/red signal.
- **Scaffolding CLI** — **`nest-rs-cli`** / binary **`nestrs`**: `nestrs new`
  (standalone + `--in-workspace`), `nestrs g feature` (port + optional `--http`),
  `nestrs doctor`. Shipped in the workspace; crates.io with the next lockstep
  release. Next: `resolver`, `entity`, `resource`, migrations, `nestrs info`.

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

# Roadmap

NestRS is **stable at 1.0**. Every `nest-rs-*` crate publishes at the same
version in lockstep, the public API follows semver, and a breaking change waits
for `2.0`. The third-party types that appear in your own code — `poem`,
`sea-orm`, `async-graphql`, `rmcp`, `inventory`, `validator`, `schemars` — have
their majors tied to the NestRS major and frozen for the whole `1.x` line: one
dependency resolution, for the life of `1.x`.

This file tracks what comes **next, on top of that base**. It is a *direction,
not a dated commitment*; priorities move with what the community needs. Sections
are ordered by priority, and `Later` holds what is explicitly deferred. Shipped
changes are recorded in [CHANGELOG.md](CHANGELOG.md); the authoritative record
of *what was decided and why* is [CLAUDE.md](CLAUDE.md).

Want to shape it? Open a
[Discussion](https://github.com/YV17labs/NestRS/discussions) or pick up a
[`good first issue`](https://github.com/YV17labs/NestRS/labels/good%20first%20issue).

## Shipped

The framework, its release pipeline, its documentation and its reference apps
are in place.

- **Transparent security across every transport.** Authn, authz, row-level
  filtering, response masking and transactions are carried by the framework and
  verified at boot. `nest-rs-mcp` carries the same guarantee as HTTP and
  GraphQL: its guard chain is deny-all by default, and the request scope,
  executor and ability reach a tool body, so a `Repo`-backed MCP tool is
  row-filtered exactly like a controller.
- **crates.io publishing** — every `nest-rs-*` framework crate is on
  [crates.io](https://crates.io/crates/nest-rs); `apps/` and the product crates
  under `demo/` stay `publish = false`.
- **Release automation** — versions move in **lockstep** (one number for the
  whole workspace, centralised in `[workspace.package]`), so a NestRS version
  names exactly one compatible resolution. Push a `v*.*.*` tag and
  `.github/workflows/publish.yml` runs an idempotent `cargo workspaces publish`
  in dependency order.
- **The `nest-rs` facade crate** — one dependency and one feature set on
  crates.io (`nest-rs`); in code, `use nest_rs::prelude::*;` for the common case.
- **The [nestrs.dev](https://nestrs.dev) docs site** — Starlight site with
  getting started, an end-to-end tutorial, fundamentals, and one section per
  surface crate (HTTP, GraphQL, security, database, queue, events, MCP, health,
  OpenTelemetry, testing, …). Published on every push to `main` via
  `.github/workflows/docs-pages.yml`. Every framework crate also ships a
  `README.md` on crates.io and docs.rs.
- **Reference apps** — the **Publish** workspace under `demo/apps/` (`auth`,
  `api`, `assistant`, `live`, `worker`), plus a multi-binary Docker image and a
  dev container with Postgres and Redis. Simple hello/blog layouts are
  CLI-scaffolded — documented at [nestrs.dev](https://nestrs.dev), not hosted
  here.
- **Scaffolding CLI** — **`nest-rs-cli`** / binary **`nestrs`**: `nestrs new`
  (workspace by default, `--standalone` for a single crate), `nestrs g feature`
  (transport-agnostic port), the `g resource` / `http` / `graphql` / `ws` /
  `queue` / `schedule` / `mcp` / `migration` generators, and `nestrs doctor` —
  on [crates.io](https://crates.io/crates/nest-rs-cli)
  (`cargo install --locked nest-rs-cli`).
- **GitHub organisation** — canonical home at
  [github.com/YV17labs/NestRS](https://github.com/YV17labs/NestRS). The repo
  stays a **single monorepo** — the model every multi-crate Rust framework uses
  (`tokio`, `bevy`, `axum`) — so one atomic commit can span a crate, its
  `*-macros` companion, and an example app.

## Next — moving checks earlier

The framework's guarantees hold today. Each item below takes a check that
already **fails closed at run time** and moves it to compile or boot time, which
is where NestRS prefers to answer a wiring question.

- **Compile-time handles for the last stringly-typed seams** — typed queue
  handles ship and are the default (`QueueName`, `#[queue]`, `push_to`). Next:
  deprecating the legacy string-literal `#[process(queue = "…")]` form, and
  giving the dataloader loader-type (resolved today by naming convention, e.g.
  `UsersServiceByName`) a typed handle, so the two macros stop agreeing on a
  string recipe.
- **Alias-proof masking arm** — `#[routes]` arms the response shaper (ambient
  ability + masking) by matching a parameter path segment named
  `Authorize`/`Bind`. A renamed import (`use Authorize as Az`) **fails closed**:
  unarmed routes carry a `MaskProbe`, and a masking extractor running without an
  armed shaper turns the response into a logged `500` rather than an unmasked
  body. Next: a generic ambient-context seam in `nest-rs-http` (the extractor
  registers a type-erased masker + ability; a generic shaper applies them) so
  arming no longer depends on how the type is spelled.

## Next — extending shipped features

Additions to capabilities that already ship. Each is an extension of a working
surface, not a prerequisite for using it.

- **OpenAPI** — the document carries typed path params, expanded query params,
  `bearerAuth` on guarded routes, per-route error statuses (the effective
  success code plus `400`/`404`/`429`), RFC 9457 error responses and a
  boot-written `openapi.json` snapshot. Next: header parameters, and schemas for
  multipart request bodies and streamed responses.
- **File storage** — `nest-rs-storage` ships the presign, head and byte surface.
  Next: delete, list, and streaming uploads.
- **Dependency-injection scopes** — request and transient scopes ship on HTTP
  via `Scoped<T>`, including one level of request-scoped → request-scoped
  dependencies, and on MCP via `PropagatingHandler`. Next: request-scoped chains
  deeper than one level, and bridging the scope into the GraphQL request path
  (which carries per-request state through its own context and DataLoaders).
- **`nest-rs-resource`** — next: a first-class `#[expose]` enum mode (an enum
  column already passes through if it derives the surface traits), HasMany
  pagination via `Connection<T>` (the auto-emitted resolver returns a `Vec<T>`
  capped at `RELATION_LOAD_CAP = 100` per parent — bounded, but not
  cursor-paginated), and a `via = "..."` override so non-conventional FK columns
  work without a manual `#[field_resolver]`.
- **API versioning strategies** — URI versioning
  (`#[controller(version = "1")]`) ships. Next: header- and media-type-based
  selection, which need request-time dispatch.
- **TLS certificate hot-reload** — `HttpTransport::tls` loads the certificate at
  boot. Next: watching the PEM source and swapping the `rustls` config live, so
  a renewal lands without a restart.
- **Streaming uploads into storage** — multipart uploads, streamed download
  bodies, SSE and response compression all ship (poem's `Multipart`,
  `Body::from_bytes_stream`, `SSE`, `HttpConfig.compression`), bounded by the
  transport-wide `max_body_bytes` cap that covers every extractor. Next:
  streaming a multipart *part* straight into storage without buffering it whole.

## Next — additional building blocks

Well-understood server primitives worth owning, listed because they are
*load-bearing for real use*. The verdict on what is **not** worth reproducing is
in *Not on the roadmap*.

- **Caching** — a `CacheModule` + a response-caching interceptor + an injectable
  `Cache` provider, memory- or Redis-backed.

## Next — tooling

- **Docs tooling** — CI-verified code snippets and the Basics → All options tier
  split per section.
- **CLI generators** — a dedicated `entity` generator (reachable today via
  `g resource`) and a `nestrs info` command (`nestrs about` already covers most
  of it).

## Later — deferred

Not current priorities; these follow when a real app needs them.

- **Transport-neutral guard core** — one `Guard` trait, one chain and one
  dispatch across every transport is a deliberate `1.x` design. `check_http`
  sits on the base trait, so `nest-rs-guards` links the HTTP stack even in a
  headless binary — a binary-size question with no runtime, security or
  correctness effect. `check_graphql`/`check_ws_message` are already
  feature-gated; moving `check_http` into an `HttpGuard` extension trait touches
  every guard impl and the HTTP dispatch, so it lands in a major.
- **Per-job transactions** — a `#[cron_job]`/`#[processor]` runs on the
  connection **pool**: a worker job has no safe/mutating method to classify, the
  way an HTTP verb or a WebSocket message does. Deliberately deferred.
- **Server-Sent Events & GraphQL subscriptions** — `@Sse` and a subscription
  root; both would reuse the WebSocket gateway's per-connection plumbing, which
  is where realtime lives today.
- **gRPC** and other request/response transports, as the discovery model proves
  out.
- GraphQL **federation**, and the dedicated schema tooling it would reintroduce.

## Not on the roadmap

By design — see the *Hard "no" list* in [CLAUDE.md](CLAUDE.md):

- No external dependency-injection library — the container is ours.
- No splitting the workspace into microservices "for scalability".
- No backwards-compatibility shims — a breaking change waits for the next major.
- **No `ClassSerializerInterceptor` / `@Exclude` / `@Expose`** — serde already owns
  serialization (`#[serde(skip)]`, or a dedicated response DTO); a per-request
  "groups" interceptor is not worth reproducing.
- **No `HttpModule` / `HttpService`** — inject a configured `reqwest::Client`; an
  axios-style wrapper would be pure ceremony.
- **No bundled `Logger` service** — `tracing` is the idiomatic, structured, superior
  answer, and is already the project's logging layer.

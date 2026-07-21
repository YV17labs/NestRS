# Roadmap

This is a *direction, not a dated commitment*; priorities move with what the
community needs. The sections below are ordered by **priority** — correctness and
parity work first; `Later` holds what is explicitly deferred.

Want to shape it? Open a
[Discussion](https://github.com/YV17labs/NestRS/discussions) or pick up a
[`good first issue`](https://github.com/YV17labs/NestRS/labels/good%20first%20issue).
The authoritative record of *what was decided and why* is
[CLAUDE.md](CLAUDE.md); this file tracks *what's next*.

## Now — the road to v1.0

The single active priority is a **stable 1.0**, and the stabilisation work is
now code-complete. The shape below is what has landed and what remains before
the tag:

- **The stabilisation code work has landed** — the security blockers (throttler
  `X-Forwarded-For` keying, an unbounded request-body path, a silently-dying
  scheduler, a per-request panic for one DI shape), the fail-closed parity and
  data-integrity fixes, and API-freeze hygiene (`#[non_exhaustive]`, opaque
  structs, a declared pinned-major policy for `poem`/`sea-orm`/`async-graphql`)
  are all in with their proving tests. What remains before the tag is the
  version bump and the release itself.
- **Three crates carry an explicit disposition**: `nest-rs-storage` stays
  path-only, excluded from the 1.0 publish (no delete/list/streaming);
  `nest-rs-mcp` ships with its guard chain working (deny-all default) but
  transparent row-level filtering inside a tool deferred past 1.0 and
  documented; `nest-rs-openapi` is matured — the generated document now states
  the wire contract, with header parameters and a multipart body schema the
  only remaining gaps.

## Next — hardening the guarantees

The framework's promises — transparent security, a DI graph checked at boot,
declarative wiring — hold today but lean on developer discipline at a few seams.
Closing these is what makes the guarantees *airtight*, the real edge over a
framework that only **documents** the same concerns. The v1-blocking subset has
already landed; what stays here is the longer tail beyond 1.0.

- **Compile-time guardrails for the stringly-typed seams** — typed queue
  handles now ship and are the default (`QueueName`, `#[queue]`, `push_to`);
  the residual work is deprecating the legacy string-literal `#[process(queue
  = "…")]` form and giving the dataloader loader-type (found today by naming
  convention, e.g. `UsersServiceByName`) a typed handle so the two macros stop
  agreeing on a string recipe.
- **Alias-proof masking arm** — `#[routes]` arms the response shaper (ambient
  ability + masking) by *textually* matching a parameter path segment named
  `Authorize`/`Bind`. A renamed import (`use Authorize as Az`) now **fails
  closed at run time**: unarmed routes carry a `MaskProbe`, and a masking
  extractor running without an armed shaper turns the response into a logged
  `500` instead of an unmasked body. What remains is the compile/boot-time
  version: a generic ambient-context seam in `nest-rs-http` (the extractor
  registers a type-erased masker + ability; a generic shaper applies them) so
  arming stops depending on how the type is spelled.
- **Transport-neutral guard core** — the base `Guard` trait requires
  `check_http(&mut poem::Request)` and `nest-rs-guards` depends on the HTTP
  stack, so a worker-only binary compiles HTTP it never serves. **Accepted for
  1.x** (one trait, one chain, no duplicated dispatch — see the crate docs).
  `check_graphql`/`check_ws_message` are already feature-gated; the residual is
  moving `check_http` off the base trait into an `HttpGuard` extension so a
  headless binary carries no `poem`. This is a build-hygiene split with no
  runtime effect, and doing it cleanly touches every guard impl and the HTTP
  dispatch — deferred past 1.0 rather than reworked late in stabilization. It
  freezes the trait HTTP-coupled for the 1.x line.
- **Transparent row-level filtering on MCP** — rmcp executes a `#[tool]` body
  inside its own spawned `serve_inner` loop, so the executor/ability task-locals
  installed around the endpoint never reach the tool. The guard chain works
  (deny-all default, 401 without a token); making `Repo`-backed tools transparent
  needs executor + ability re-installed *inside* rmcp's dispatch (wrapping the
  generated `call_tool`), the same guarantee the other three transports already
  carry.

## Next — completing shipped features

Known, deliberate gaps in features that already ship:

- **OpenAPI last edges** — the document now carries typed path params, expanded
  query params, `bearerAuth` on guarded routes, per-route error statuses (the
  effective success code plus `400`/`404`/`429`), RFC 9457 error responses and a
  boot-written `openapi.json` snapshot. What remains: header parameters and a
  schema for multipart request bodies and streamed responses (both advertise no
  body today).
- **Dependency-injection scopes** — request and transient scopes ship on HTTP via
  `Scoped<T>`, including one level of request-scoped → request-scoped
  dependencies. What remains is request-scoped chains deeper than one level, and
  bridging the scope into the GraphQL and MCP request paths (which carry
  per-request state through their own context / DataLoaders for now).
- **`nest-rs-resource`** — a first-class `#[expose]` enum mode (an enum column
  already passes through if it derives the surface traits), HasMany pagination
  via `Connection<T>` (the auto-emitted resolver returns a raw `Vec<T>` today,
  capped at `RELATION_LOAD_CAP = 100` per parent — safe, but not cursor-
  paginated), and a `via = "..."` override for
  HasMany so non-conventional FK columns work without falling back to a manual
  `#[field_resolver]`. Adopter backlog landed: HTTP-only `#[expose]`, opt-in
  `soft_delete`/`timestamps`, relation-graph diagnostics, and a guard-naming doc.
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

- **Caching** — a `CacheModule` + a response-caching interceptor + an injectable
  `Cache` provider, memory- or Redis-backed.
- **Streaming uploads** — multipart uploads, streamed download bodies, SSE and
  response compression all ship (poem's `Multipart`, `Body::from_bytes_stream`,
  `SSE`, `HttpConfig.compression`); what remains is streaming a multipart *part*
  into storage without buffering it whole per field. (The transport-wide
  `max_body_bytes` cap that bounds every extractor — bare `Json`/`Multipart`
  included — now ships.)

## Shipped — project & release infrastructure

Landed with the first `0.1.0` crates.io release and the first docs push:

- **crates.io publishing** — every publishable `nest-rs-*` framework crate is on
  [crates.io](https://crates.io/crates/nest-rs); `apps/` and product crates — plus
  `nest-rs-storage`, held out of the first release — stay `publish = false`.
- **Release automation** — versions move in **lockstep** (one number for the whole
  workspace, centralised in `[workspace.package]`). Push a `v*.*.*` tag and
  `.github/workflows/publish.yml` runs an idempotent `cargo workspaces publish`
  in dependency order. Independent per-crate versioning waits until crates
  stabilise at different rates.
- **The `nest-rs` facade crate** — one dependency and one feature set on
  crates.io (`nest-rs`); in code, `use nest_rs::prelude::*;` for the common case.
- **GitHub organisation** — canonical home at
  [github.com/YV17labs/NestRS](https://github.com/YV17labs/NestRS).
- **The [nestrs.dev](https://nestrs.dev) docs site** — Starlight site with getting
  started, an end-to-end tutorial, fundamentals, and one section per surface crate
  (HTTP, GraphQL, security, database, queue, events, MCP, health, OpenTelemetry,
  testing, …). Published on every push to `main` via
  `.github/workflows/docs-pages.yml`.
- **Reference apps** — the **Publish** workspace under `demo/apps/`
  (`auth`, `api`, `assistant`, `live`,
  `worker`), plus a multi-binary Docker image and a dev container
  with Postgres and Redis. Simple hello/blog layouts are CLI-scaffolded
  only — documented at [nestrs.dev](https://nestrs.dev), not hosted here.
- **Crate-level READMEs** — every framework crate ships a minimal `README.md`
  (description + links to [nestrs.dev](https://nestrs.dev) and GitHub);
  extension-point contracts live in the docs site.
- **Scaffolding CLI** — **`nest-rs-cli`** / binary **`nestrs`**: `nestrs new`
  (workspace by default, `--standalone` for a single crate), `nestrs g feature`
  (transport-agnostic port), the `g resource` / `http` / `graphql` / `ws` /
  `queue` / `schedule` / `mcp` generators, and `nestrs doctor` — published on
  [crates.io](https://crates.io/crates/nest-rs-cli) with the `0.3.0` lockstep
  release (`cargo install --locked nest-rs-cli`).

## Next — project & release infrastructure

What remains before the workspace is easy to adopt and contribute to. The repo
stays a **single monorepo** (the model every multi-crate Rust framework uses —
`tokio`, `bevy`, `axum`): one atomic commit can span a crate, its `*-macros`
companion, and an example app, which a repo-per-crate split would make impossible.

- **Polish the `docs/` site** — CI-verified code snippets, the Basics → All options
  tier split per section, and keeping pages aligned as the API shifts.
- **Scaffolding CLI — remaining generators** — the shipped `nestrs g` surface
  (see *Shipped*) now includes `migration`; what remains is a dedicated
  `entity` generator (available today only via `g resource`) and a `nestrs
  info` command (`nestrs about` already covers most of it).

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

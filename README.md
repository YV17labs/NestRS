<p align="center">
  <img src="assets/wordmark.svg" alt="NestRS" width="220">
</p>

<p align="center">
  <strong>NestJS ergonomics, Rust performance.</strong><br>
  A declarative, decorator-driven backend framework that compiles to a single
  native binary — a fraction of the memory, none of the garbage-collector tax.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/built%20with-Rust-CE412B?logo=rust&logoColor=white" alt="Built with Rust">
  <img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT License">
  <img src="https://img.shields.io/badge/status-alpha-orange" alt="Status: alpha">
  <img src="https://img.shields.io/badge/PRs-welcome-brightgreen" alt="PRs welcome">
</p>

> [!NOTE]
> **Alpha — under active development.** The API still shifts and rough edges
> remain, so it is not production-ready yet. Stars and early feedback are very
> welcome.

## Why NestRS

- ⚡ **Rust-native speed.** Built on the same hyper/tokio core as the fastest Rust
  web frameworks — multiples of a Node service's throughput, no GC pauses, and
  tail latencies that stay flat under load.
- 🪶 **An order of magnitude less memory.** A footprint in the tens of MB, not
  hundreds — smaller instances, higher density, a lighter cloud bill.
- 🚀 **Boots in milliseconds.** A single static native binary with no runtime to
  warm up — friendly to autoscaling and cold starts.
- 🧩 **Declarative by design.** `#[module]`, `#[controller]`, `#[injectable]`,
  `#[resolver]`, `#[processor]` — features are wired with attribute macros, not
  hand-written boilerplate.
- 🛡️ **Verified before it serves.** The DI graph is wired by macros and checked at
  boot — no reflection, no runtime surprises.
- 📦 **Batteries included.** HTTP, GraphQL, OpenAPI, MCP, Redis-backed queues,
  scheduling, CASL-style authorization, health probes and OpenTelemetry — each an
  opt-in crate, so you compile only what you import.

<sub>Performance figures describe typical native-Rust-vs-Node behaviour; NestRS's own published benchmarks are on the way.</sub>

## Vision

A few trends made this project feel worth trying.

Memory has become a serious cost. Provisioning RAM in the cloud has grown much
more expensive in recent years, and for many services it is now the largest part
of the bill. Managed runtimes — Node among them — are genuinely productive, but
they reach that productivity through a runtime and a garbage collector with a
sizeable, always-resident footprint, which also means more energy spent per
request.

At the same time, LLM-assisted coding has lowered the barrier to writing native,
lower-level code. Much of the friction that made higher-level runtimes
attractive — boilerplate, slower scaffolding, a steeper learning curve — is
easier to absorb today, regardless of the language.

That is the trade-off NestRS reopens: keep the declarative, decorator-driven
style that makes that model productive, but build it on a native, compiled
foundation that doesn't bill you for it in RAM. One `cargo` step compiles and
type-checks, modules wire up regardless of import order, and the result ships as
one lean binary. It's young and moving fast — the ambition is real, the polish is
still arriving.

## Project layout

NestRS is a **Cargo workspace** — one repository holding many crates, built and
versioned together. Two kinds of member live in it:

- **Applications** under [`apps/`](apps/) — each is a binary crate you run and
  deploy on its own (`api`, `app`, `mcp`, `worker`). One repository, several
  independently shippable services.
- **Libraries** under [`crates/`](crates/) — ordinary library crates of reusable
  code. The framework itself ships this way (`nestrs-core`, `nestrs-http`,
  `nestrs-graphql`, …), and any code you want to share across your apps becomes a
  crate here too.

```
nestrs/
├─ apps/            applications — one runnable binary each
│  ├─ api/          REST + GraphQL, persisted & authorized
│  ├─ app/          minimal HTTP baseline
│  ├─ mcp/          Model Context Protocol server
│  └─ worker/       background jobs & scheduling (headless)
└─ crates/          libraries — the framework, plus your shared code
   ├─ nestrs-core/  IoC container, modules, DI, bootstrap
   ├─ nestrs-http/  REST controllers & routing
   └─ …             one crate per capability
```

Adding an application means adding a directory under `apps/`; factoring out
shared code means adding one under `crates/`. The workspace picks both up
automatically (`members = ["crates/*", "apps/*"]`) — no central manifest to edit,
and the release image auto-discovers every app binary.

## What's included

Capabilities ship as separate crates, so an app compiles only what it imports
(the headless `worker` pulls in neither HTTP nor GraphQL). The developer-facing
surface is decorator macros — reach for them first (`#[injectable]`, `#[module]`,
`#[controller]`, `#[resolver]`, `#[processor]`, …).

| Crate | What it gives you |
|-------|-------------------|
| `nestrs-core` | IoC container, modules (`#[module]`), DI (`#[injectable]`), lifecycle hooks (`#[hooks]`), app bootstrap, boot-time module access-graph check |
| `nestrs-config` | Typed config from env + TOML (`NESTRS_<DOMAIN>__<KEY>` scheme) |
| `nestrs-http` | REST controllers (`#[controller]`/`#[routes]`), per-verb routing, route guards (`#[use_guards]`); poem-backed |
| `nestrs-graphql` | Resolvers (`#[resolver]`/`#[query]`/`#[mutation]`/`#[field]`), self-composing schema, request-scoped dataloaders (`#[dataloader]`) |
| `nestrs-openapi` | OpenAPI 3.1 document + bundled offline Swagger UI, composed from the route table |
| `nestrs-mcp` | Model Context Protocol server over Streamable-HTTP (`#[mcp]`), `rmcp`-backed |
| `nestrs-orm` | SeaORM database module — async pool via `DatabaseModule::for_root` |
| `nestrs-queue` | Redis-backed durable job queues + workers (`#[processor]`); `apalis`-backed |
| `nestrs-schedule` | In-process cron / interval jobs (`#[cron_job]`) |
| `nestrs-authz` | CASL-style authorization: one ability → access gate + query pre-filter + response masking (HTTP binding in `nestrs-authz-http`) |
| `nestrs-pipes` | Transport-agnostic validation & transformation (`ValidationPipe`, `Parse*`, …) |
| `nestrs-middleware` | Guards, interceptors, exception filters |
| `nestrs-resource` | Expose a SeaORM entity to GraphQL **and** OpenAPI from one `#[expose]` |
| `nestrs-health` | Kubernetes liveness / readiness / startup probes |
| `nestrs-telemetry` | Structured logs, OpenTelemetry traces & metrics, per-request access log + `X-Trace-Id` |
| `nestrs-server-timing` | `Server-Timing` response headers |

Decorator macros live in companion `*-macros` crates (a Rust `proc-macro` crate
can export only macros) with shared codegen in `nestrs-codegen`; these are
internal plumbing, re-exported by the crates above and never depended on directly.

## Getting started

### In a dev container (recommended)

The repo ships a [dev container](.devcontainer/) — the fastest path to a working
setup on any machine with Docker and a devcontainer-aware editor.

1. Install [Docker](https://docs.docker.com/get-docker/) and the VS Code
   [Dev Containers](https://marketplace.visualstudio.com/items?itemName=ms-vscode-remote.remote-containers)
   extension.
2. Open the repo in VS Code and run **Dev Containers: Reopen in Container** (or
   accept the prompt VS Code shows on open).

That is the whole setup. The container provisions the Rust toolchain and the dev
tooling (`just`, `bacon`, `cargo-nextest`, …), and brings up **Postgres** and
**Redis** beside it with `DATABASE_URL` / `REDIS_URL` already pointed at them — so
`api` and `worker` run with no further configuration. Ports 3001–3003 are
forwarded to the host.

Then start an app in watch mode:

```bash
just dev api      # REST + GraphQL on :3002
just dev mcp      # MCP server on :3003
just dev worker   # background jobs & scheduling (headless)
just dev          # the bare `app` baseline on :3001
```

`just dev` runs under `bacon`, which rebuilds and restarts the binary on every
save — edit a handler, save, and the change is live (`mold` is wired in as the
linker to keep incremental rebuilds fast). Leave it running in a terminal while
you work.

### On your own machine

Prefer a local toolchain? Install Rust 1.75 or newer (https://rustup.rs) and the
dev tooling:

```bash
cargo install --locked just bacon cargo-nextest cargo-llvm-cov
rustup component add llvm-tools-preview
```

| Tool | Purpose |
|------|---------|
| [`just`](https://github.com/casey/just) | Task runner — recipes for the common workflows |
| [`bacon`](https://dystroy.org/bacon/) | Watcher — rebuilds and restarts on save |
| [`cargo-nextest`](https://nexte.st) | Parallel test runner, noticeably faster than `cargo test` |
| [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) | Source-based test coverage (uses LLVM, plays well with nextest) |

The `api` app needs Postgres and the `worker` needs Redis; run your own and
export `DATABASE_URL` / `REDIS_URL` (the `app` and `mcp` binaries need neither).

## Commands

Run `just` with no arguments to list every recipe.

| Command | What it does |
|---------|--------------|
| `just dev <app>` | Run an app in watch mode (rebuild + restart on change), e.g. `just dev api` or `just dev mcp` |
| `just run <app>` | Run an app in release mode, e.g. `just run api` |
| `just build` | Build release binaries for every app in the workspace |
| `just test` | Run the full test suite |
| `just cov` | Test coverage summary (per-file %) |
| `just lint` | Clippy (strict) + format check |
| `just fmt` | Apply rustfmt |
| `just check` | Fast type-check (no codegen) |

`build`, `test`, `cov`, `lint`, `fmt`, and `check` always operate on the whole
workspace; `dev` and `run` take an app name (default `app`).

## Applications

### `api` — REST + GraphQL, persisted and authorized (port 3002)

Started with `just dev api`. Persists to **Postgres** through SeaORM
(`nestrs-orm`), so it needs a `DATABASE_URL` (e.g.
`postgres://postgres:postgres@localhost/nestrs`) — boot aborts with a clear
message if it is unset. Listens on `http://0.0.0.0:3002`:

| Endpoint | Purpose |
|----------|---------|
| `GET  /users`, `GET /users/:id`, `POST /users` | REST resource |
| `POST /graphql` | GraphQL endpoint |
| `GET  /graphql` | GraphQL playground |
| `GET  /api-json` | OpenAPI 3.1 document |
| `GET  /api` | Swagger UI |
| `GET  /health/live` | Kubernetes liveness probe |
| `GET  /health/ready` | Kubernetes readiness probe |
| `GET  /health/startup` | Kubernetes startup probe |

Resolvers are declared with `#[resolver]`: `#[query]`/`#[mutation]` add root
fields, and `#[field]` adds a field resolver to an object type — it takes the
resolved object as `parent: &T` and reaches services through the resolver's
`#[inject]` fields. The schema composes itself from every resolver in the binary
(no central list) and is committed as SDL at
[`apps/api/schema.graphql`](apps/api/schema.graphql), so API changes surface in
diffs. A dev run of the server rewrites that file on boot (`GraphqlModule`'s
`emit_sdl` is `true` under `debug_assertions`, `false` in a release build); commit
the result after touching a resolver.

The REST surface documents itself the same way: import `OpenApiModule` and the
OpenAPI document composes from every `#[controller]` in the binary — verbs and
paths from the route table, request/response schemas from each `Json<T>` payload
(DTOs derive `schemars::JsonSchema`, the same trait MCP uses), grouped by
controller. `#[api(summary = "...", tags("..."))]` beside a verb enriches an
operation. Swagger UI is bundled and served offline at `/api` (the document at
`/api-json`).

It also exercises the full request + authorization pipeline. Each `/users` route
is bound with `#[use_guards(AuthGuard, AppAbilityGuard)]`: `AuthGuard`
authenticates (`x-api-key` + `x-org-id` headers) and attaches an `AuthUser`, then
`AppAbilityGuard` builds the caller's CASL-style `Ability` from it. That one
ability drives all three powers — the `Authorize<Action, Entity>` extractor gates
access (`403`) and masks the response to the fields and rows the caller may see,
`ability.condition_for::<Entity>(…)` pre-filters the SeaORM query to the caller's
org, and `ability.can::<Entity>(…)` makes the per-row check on by-id reads. Inputs
pass through pipes — `Valid<Json<…>>` validation and `Piped<ParseUuidV7,
Path<…>>` parsing.

### `app` — Minimal HTTP endpoint (port 3001)

Started with `just dev app`. Listens on `http://0.0.0.0:3001` with a single
`GET /` returning `Hello World`. Kept deliberately bare — no health, telemetry,
or middleware — to serve as a baseline when benchmarking the framework's
request path.

### `mcp` — Model Context Protocol server (port 3003)

Started with `just dev mcp`. Exposes a Streamable-HTTP MCP server backed by
`rmcp`, with tools declared the same way controllers are — `#[injectable]` for
DI, then `#[tool_router]` / `#[tool]` / `#[tool_handler]` on the controller.

The bundled `current_weather` tool takes GPS coordinates and queries the
[Open-Meteo](https://open-meteo.com) public forecast API. Latitude/longitude
bounds are declared with `validator` annotations on the params struct and
checked at the start of the tool handler.

The upstream HTTP client shows the async-provider pattern: a `WeatherConfig` is
seeded on `App::builder()` and an async `provide_factory` builds a
timeout-configured `reqwest::Client` from it once at boot, which the tool then
injects (override with `NESTRS_WEATHER__BASE_URL` / `NESTRS_WEATHER__REQUEST_TIMEOUT_MS`).

| Endpoint | Purpose |
|----------|---------|
| `POST /mcp` | MCP Streamable-HTTP transport |
| `GET  /health/live` | Kubernetes liveness probe |
| `GET  /health/ready` | Kubernetes readiness probe |
| `GET  /health/startup` | Kubernetes startup probe |

Point any MCP client (Claude Desktop, Cursor, custom integrations) at
`http://localhost:3003/mcp`.

### `worker` — Background jobs & scheduling (headless)

Started with `just dev worker`. No HTTP surface — it runs two transports: a
`Scheduler` (in-process cron / interval jobs) and a `QueueWorker` (Redis-backed
durable jobs via `apalis`). Needs a Redis instance (`REDIS_URL`, default
`redis://127.0.0.1/`).

The bundled `audio` feature shows the full producer → queue → consumer loop:
`AudioProducer`, a `#[cron_job(every = "5s")]`, enqueues a transcode job every
five seconds, and `AudioConsumer`, a
`#[processor(queue = "audio", concurrency = 5, retries = 3)]`, pulls and processes
it (retried on failure). Producer and consumer are decoupled by the queue name —
jobs survive a restart and any number of worker processes share one queue.

Because it enables `nestrs-telemetry` without the `http` feature and imports no
HTTP crate, the worker binary never compiles the poem stack — a genuinely lean
headless build.

## Docker

A multi-stage [`Dockerfile`](Dockerfile) at the repo root builds **every
workspace binary** into a single image. Which one runs is chosen at `docker
run` time:

```bash
docker build -t nestrs .

# Run the app on port 3001
docker run --rm -p 3001:3001 nestrs /usr/local/bin/app

# Run the default app (api) on port 3002
docker run --rm -p 3002:3002 nestrs

# Run the mcp app on port 3003
docker run --rm -p 3003:3003 nestrs /usr/local/bin/mcp
```

Adding a new app under `apps/` requires no Dockerfile change — the builder
auto-discovers every release binary and ships it.

Security defaults baked in:

- Runtime image is `gcr.io/distroless/cc-debian13:nonroot` — no shell, no
  package manager, runs as UID 65532 by default.
- `cargo-chef` cooks dependencies in a cacheable layer, so dep changes don't
  trigger a full rebuild.
- Rust version and `cargo-chef` version are pinned via build args:
  `--build-arg RUST_VERSION=1.95 --build-arg CARGO_CHEF_VERSION=0.1.77`.
- No `HEALTHCHECK` directive — use the Kubernetes probes exposed at
  `/health/{live,ready,startup}` (the right layer for orchestrator health).

## Coming from NestJS?

NestRS borrows NestJS's programming model — modules, providers, decorators,
dependency injection — and rebuilds it natively in Rust. If you already know
Nest, this is the map; otherwise you can skip it.

**Project structure.** NestJS's [monorepo mode](https://docs.nestjs.com/cli/monorepo)
(several applications in one workspace) and its [libraries](https://docs.nestjs.com/cli/libraries)
(shared code) map directly onto a Cargo workspace: applications under `apps/`,
shared libraries as crates under `crates/`. There is no `nest-cli.json` —
`cargo` is the build tool and the workspace manifest is the project definition.

**Decorators & concepts:**

| NestRS | NestJS |
|--------|--------|
| `#[module]` | `@Module()` |
| `#[injectable]` | `@Injectable()` |
| `#[controller]` / `#[routes]` + `#[get]`/`#[post]`/… | `@Controller()` + `@Get()`/`@Post()`/… |
| `#[use_guards(...)]` | `@UseGuards()` |
| `#[resolver]` + `#[query]`/`#[mutation]` | `@Resolver()` + `@Query()`/`@Mutation()` |
| `#[field]` | `@ResolveField()` |
| `#[dataloader]` | DataLoader |
| `#[cron_job]` | `@Cron()` |
| `#[processor]` | `@Processor()` |
| `#[hooks]` + `#[on_module_init]`/… | `onModuleInit()`/… lifecycle hooks |
| `ValidationPipe` / `Parse*` | pipes |

**Crates ↔ packages:**

| NestRS crate | NestJS package |
|--------------|----------------|
| `nestrs-core` | `@nestjs/core` |
| `nestrs-config` | `@nestjs/config` |
| `nestrs-http` | `@nestjs/platform-express` |
| `nestrs-graphql` | `@nestjs/graphql` |
| `nestrs-openapi` | `@nestjs/swagger` |
| `nestrs-orm` | `@nestjs/typeorm` |
| `nestrs-queue` | `@nestjs/bullmq` |
| `nestrs-schedule` | `@nestjs/schedule` |
| `nestrs-authz` | CASL / `@casl/ability` |
| `nestrs-pipes` / `nestrs-middleware` | `@nestjs/common` |
| `nestrs-health` | `@nestjs/terminus` |

**What's different on purpose:**

- **Module encapsulation is compile-time.** A module's boundary is its Rust
  visibility — no runtime `exports` list. Expose a `pub` trait, keep the impl
  private.
- **The DI graph is checked at boot**, not resolved by reflection — there is no
  `reflect-metadata` and no `forwardRef`.
- **One build step.** `cargo` compiles, type-checks, and links to a single native
  binary; there is no separate transpile pass.

## License

MIT — see [LICENSE](LICENSE).

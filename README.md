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

- ⚡ **Rust-native speed.** ~25× the throughput of an equivalent NestJS service on
  the same CPU budget (~13× per core), with a sub-millisecond p99 — built on the
  same hyper/tokio core as the fastest Rust web frameworks, with no GC pauses and
  tail latencies that stay flat under load. [See the benchmark.](#benchmark)
- 🪶 **An order of magnitude less memory.** ~4 MB idle and ~6 MB under load, versus
  ~80–120 MB for the same NestJS service — roughly 18–20× lighter, for smaller
  instances, higher density, and a lighter cloud bill.
- 🚀 **Boots in milliseconds.** A single static native binary with no runtime to
  warm up — friendly to autoscaling and cold starts.
- 🧩 **Declarative by design.** `#[module]`, `#[controller]`, `#[injectable]`,
  `#[resolver]`, `#[processor]` — features are wired with attribute macros, not
  hand-written boilerplate.
- 🛡️ **Verified before it serves.** The DI graph is wired by macros and checked at
  boot — no reflection, no runtime surprises.
- 📦 **Batteries included.** HTTP, GraphQL, OpenAPI, MCP, Redis-backed queues,
  scheduling, an event bus, CASL-style authorization, health probes,
  OpenTelemetry and an in-process test harness — each an opt-in crate, so you
  compile only what you import.

## Benchmark

The same "Hello World" HTTP service — a provider, a controller, a module —
implemented once in NestRS and once in NestJS, under an identical `wrk` load
(`GET /`, plaintext, keep-alive). On the same CPU budget NestRS served **~25×
more requests** while using **~18× less memory**.

| Metric — `GET /` plaintext      | NestRS (Rust)  | NestJS (Node 20) | Ratio  |
| ------------------------------- | -------------- | ---------------- | ------ |
| Throughput (2 cores, defaults)  | ~463k req/s    | ~18k req/s       | ~25×   |
| Throughput (1 core, per-core)   | ~212k req/s    | ~17k req/s       | ~13×   |
| Latency, p50                    | 0.13 ms        | 3.2 ms           | ~24×   |
| Latency, p99                    | 0.57 ms        | 6.4 ms           | ~11×   |
| Memory, idle                    | 4 MB           | 80 MB            | ~20×   |
| Memory, under load              | 6 MB           | 118 MB           | ~18×   |

<sub><b>Machine:</b> a single dev container with <b>4 cores and 8 GiB RAM</b>
(aarch64, Debian 13) — both the total memory and the core count are the
container's, not the host's. <b>Method:</b> server pinned to half the cores, the
<code>wrk</code> client (<code>-t2 -c64 -d20s</code>) to the other half; median of
3 runs over loopback. NestRS is a release build on its default multi-threaded
tokio runtime; NestJS 11 runs on Express, <code>NODE_ENV=production</code>, logging
off, as a single process — the Node default, which is why it cannot use the second
core (the per-core row is the apples-to-apples figure). Loopback on a shared host
favours absolute numbers over a public leaderboard; treat these as order-of-
magnitude, and reproduce them with the <code>app</code> example.</sub>

## What the code looks like

The `app` example is a complete HTTP service — a provider, a controller that
injects it by type, and a module that wires them together. This is the whole
feature:

```rust
use std::sync::Arc;
use nestrs_core::{injectable, module};
use nestrs_http::{controller, routes};

// A provider — anything injectable.
#[injectable]
#[derive(Default)]
pub struct HelloService;

impl HelloService {
    pub fn greeting(&self) -> &'static str {
        "Hello World"
    }
}

// A controller; the service is injected by type, no token to declare.
#[controller(path = "/")]
pub struct HelloController {
    #[inject]
    svc: Arc<HelloService>,
}

#[routes]
impl HelloController {
    #[get("/")]
    async fn hello(&self) -> &'static str {
        self.svc.greeting()
    }
}

// A module groups providers; import order never matters.
#[module(providers = [HelloService, HelloController])]
pub struct HelloModule;
```

Compose modules and boot with one transport:

```rust
use nestrs_core::{module, App};
use nestrs_http::HttpTransport;

#[module(imports = [HelloModule])]
pub struct AppModule;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    App::new::<AppModule>()?
        .transport(HttpTransport::new().bind("0.0.0.0:3001"))
        .run()
        .await
}
```

`just dev` runs it; `GET /` returns `Hello World`. No reflection, no separate
codegen step — `cargo` compiles it to a single native binary, and the DI graph
is checked at boot.

The same inject-and-decorate model carries every surface, not just HTTP. The
`worker` example pairs a scheduled producer with a durable, Redis-backed
consumer — each a struct that injects what it needs and implements one trait
method for its logic:

```rust
// Runs every 5s — an in-process scheduled job.
#[cron_job(every = "5s")]
pub struct AudioProducer {
    #[inject]
    queue: Arc<QueueConnection>,
}

// A durable queue consumer — 5 jobs in flight, retried 3× on failure.
#[processor(queue = "audio", concurrency = 5, retries = 3)]
pub struct AudioConsumer {
    #[inject]
    transcoder: Arc<Transcoder>,
}
```

GraphQL resolvers (`#[resolver]`/`#[query]`), MCP tools (`#[mcp]`) and the rest
follow the same shape. The richest example, `api`, stacks REST + GraphQL +
OpenAPI behind route guards, validation pipes and request-scoped dataloaders —
see [`apps/api`](apps/api/).

## How it compares

NestRS sits *on top of* the same `hyper`/`tokio`/`poem` stack the leading Rust
web frameworks use — it doesn't replace them, it gives them structure.

- **vs. Axum / Actix / Poem** — those are (excellent) HTTP layers. You bring your
  own dependency injection, module boundaries, validation, GraphQL, OpenAPI,
  queues and scheduling, then wire them together. NestRS ships that opinionated
  structure as one coherent set of macros, so a large codebase stays declarative
  instead of growing a bespoke wiring layer.
- **vs. Loco** — Loco is the closest in spirit: opinionated and batteries-included,
  but Rails/MVC-flavoured and built around an ActiveRecord-style model. NestRS
  follows the modules-and-providers lineage instead — a DI container, compile-time
  module encapsulation, and per-surface decorator macros (HTTP, GraphQL, MCP,
  queues). Pick the mental model you'd rather think in.
- **vs. a standalone DI crate** — NestRS's container isn't bolted on; it's the
  spine the module system, lifecycle hooks, and every transport are built around,
  and the whole wiring is verified as a graph at boot.

If you like assembling your own stack, you may not want the opinions. If you want
a framework that makes the structural decisions for you — the way NestJS, Spring,
or Rails do — that's the gap NestRS fills.

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
│  ├─ db/           shared-database migrations & seeding (CLI)
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
| `nestrs-events` | Typed in-process event bus + `#[event_handler]` (the `@nestjs/event-emitter` analog) |
| `nestrs-authz` | CASL-style authorization: one ability → access gate + query pre-filter + response masking (HTTP binding in `nestrs-authz-http`, GraphQL in `nestrs-authz-graphql`) |
| `nestrs-pipes` | Transport-agnostic validation & transformation (`ValidationPipe`, `Parse*`, …) |
| `nestrs-middleware` | Guards, interceptors, exception filters |
| `nestrs-resource` | Expose a SeaORM entity to GraphQL **and** OpenAPI from one `#[expose]` |
| `nestrs-health` | Kubernetes liveness / readiness / startup probes |
| `nestrs-telemetry` | Structured logs, OpenTelemetry traces & metrics, per-request access log + `X-Trace-Id` |
| `nestrs-server-timing` | `Server-Timing` response headers |
| `nestrs-testing` | In-process test harness — boot the real DI graph and drive HTTP / GraphQL / headless transports in `cargo test`, with provider overrides and fixtures (ephemeral Postgres, telemetry) |

Decorator macros live in companion `*-macros` crates (a Rust `proc-macro` crate
can export only macros) with shared codegen in `nestrs-codegen`; these are
internal plumbing, re-exported by the crates above and never depended on directly.

Most of the table runs in the example apps today, and every app ships an
end-to-end test built on `nestrs-testing`; a couple of newer crates
(`nestrs-events`, `nestrs-authz-graphql`) ship with their own tests but are not
yet wired into an example app — doing so is a good first contribution. The rough edges and deliberately-deferred gaps (cron expressions,
OpenAPI security schemes, GraphQL federation) are tracked in the open
[roadmap](ROADMAP.md) — nothing here is a hidden TODO.

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
**Redis** beside it with `DATABASE_URL` / `REDIS_URL` already pointed at them.
`worker` then runs as-is; `api` needs its schema applied once first — `just db up`
(or `just db reset` to also load demo data). Ports 3001–3003 are forwarded to the
host.

Then start an app in watch mode:

```bash
just dev          # the bare `app` baseline on :3001
just dev api      # REST + GraphQL on :3002
just dev mcp      # MCP server on :3003
just dev worker   # background jobs & scheduling (headless)
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
| `just test` | Run all tests |
| `just test-e2e` | Run e2e tests |
| `just test-unit` | Run unit tests |
| `just test-cov` | Run coverage |
| `just lint` | Clippy (strict) + format check |
| `just fmt` | Apply rustfmt |
| `just check` | Fast type-check (no codegen) |
| `just db <verb>` | Manage the shared database: `up`, `down`, `fresh`, `status`, `seed`, `reset` (e.g. `just db up`, then `just db seed`) |

`build`, `test`, `test-cov`, `lint`, `fmt`, and `check` always operate on the whole
workspace; `dev` and `run` take an app name (default `app`); `just db` (run bare
to list the verbs) manages the shared Postgres schema and seed data.

## Example applications

The crates under `apps/` are **examples**, not products — each is a different
*kind* of application, there to show that several can share one workspace and the
same building blocks. They will grow over time.

| App | Kind | Port |
|-----|------|------|
| `app` | Minimal HTTP baseline | 3001 |
| `api` | REST + GraphQL, persisted & authorized | 3002 |
| `db` | Shared-database migrations & seeding (CLI) | — |
| `mcp` | Model Context Protocol server | 3003 |
| `worker` | Background jobs & scheduling (headless) | — |

### `app` — Minimal HTTP endpoint (port 3001)

Started with `just dev app`. A single `GET /` returning `Hello World` on
`http://0.0.0.0:3001`, kept deliberately bare — no health, telemetry, or
middleware — as a baseline for benchmarking the framework's request path.

### `api` — REST + GraphQL, persisted and authorized (port 3002)

Started with `just dev api`; persists to Postgres via SeaORM, so it needs a
`DATABASE_URL` (boot aborts with a clear message if it is unset). The schema is
applied by the `db` app, not the running service — run `just db up` once first
(or `just db reset` to also load demo users). Listens on `http://0.0.0.0:3002`:

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

It exercises most of the framework at once: a GraphQL schema that composes
itself from every `#[resolver]` in the binary (committed as SDL at
[`apps/api/schema.graphql`](apps/api/schema.graphql) so API changes show up in
diffs), an OpenAPI document that composes itself from every `#[controller]` with
a bundled offline Swagger UI at `/api`, and a full request pipeline — route
guards for authentication and CASL-style authorization (one ability drives access
gating, query pre-filtering, and response masking), with validation pipes on the
inputs.

### `db` — Shared-database migrations & seeding

The workspace shares one Postgres database, so its schema and seed live in a
single app rather than any one service. It ships two binaries — `migrate`
(SeaORM's migration runner) and `seed` (demo data) — driven through `just db`:
`just db up` applies pending migrations, `just db fresh` rebuilds from scratch,
`just db seed` loads demo data, and `just db reset` does a clean rebuild then
seed. Both binaries ship in the container image alongside the apps, so the same
image migrates and serves.

### `mcp` — Model Context Protocol server (port 3003)

Started with `just dev mcp`. A Streamable-HTTP MCP server (`rmcp`-backed) whose
tools are declared like controllers — `#[mcp]` handles DI and mounts the
server, then `#[tool_router]` / `#[tool]` / `#[tool_handler]` define the tools.
The bundled `current_weather`
tool queries the [Open-Meteo](https://open-meteo.com) public API, with
`validator` bounds on its GPS params. Point any MCP client (Claude Desktop,
Cursor, …) at `http://localhost:3003/mcp`.

### `worker` — Background jobs & scheduling (headless)

Started with `just dev worker`. No HTTP surface — it runs a `Scheduler`
(in-process cron / interval jobs) and a `QueueWorker` (Redis-backed durable jobs
via `apalis`), so it needs a `REDIS_URL`. The bundled `audio` feature shows the
full producer → queue → consumer loop with `#[cron_job]` and `#[processor]`.
Importing no HTTP crate, the binary never compiles the poem stack — a genuinely
lean headless build.

## Docker

A multi-stage [`Dockerfile`](Dockerfile) at the repo root builds **every
workspace binary** into a single image. Which one runs is chosen at `docker
run` time:

```bash
docker build -t nestrs .

# Run the default app (the `app` baseline) on port 3001
docker run --rm -p 3001:3001 nestrs

# Run the api app on port 3002
docker run --rm -p 3002:3002 nestrs /usr/local/bin/api

# Run the mcp app on port 3003
docker run --rm -p 3003:3003 nestrs /usr/local/bin/mcp

# Apply migrations (and optionally seed) with the same image
docker run --rm nestrs /usr/local/bin/migrate up
docker run --rm nestrs /usr/local/bin/seed
```

Adding a new app under `apps/` requires no Dockerfile change — the builder
auto-discovers every release binary and ships it.

Security defaults baked in:

- Runtime image is `gcr.io/distroless/cc-debian13:nonroot` — no shell, no
  package manager, runs as UID 65532 by default.
- `cargo-chef` cooks dependencies in a cacheable layer, so dep changes don't
  trigger a full rebuild.
- No `HEALTHCHECK` directive — use the Kubernetes probes exposed at
  `/health/{live,ready,startup}` (the right layer for orchestrator health).

## Coming from NestJS?

NestRS borrows NestJS's programming model — modules, providers, decorators,
dependency injection — and rebuilds it natively in Rust. If you already know
Nest, this is the map; otherwise you can skip it.

**Project structure.** NestJS's monorepo mode (several applications in one
workspace) and its libraries (shared code) map directly onto a Cargo workspace:
applications under `apps/`, shared libraries as crates under `crates/`. There is
no `nest-cli.json` — `cargo` is the build tool and the workspace manifest is the
project definition.

**Decorators & concepts:**

| NestRS | NestJS |
|--------|--------|
| `#[module]` | `@Module()` |
| `#[injectable]` | `@Injectable()` |
| `#[controller]` / `#[routes]` + `#[get]`/`#[post]`/… | `@Controller()` + `@Get()`/`@Post()`/… |
| `#[use_guards(...)]` | `@UseGuards()` |
| `#[meta(...)]` + `Reflector` | `@SetMetadata()` / `@Roles()` + `Reflector` |
| `#[resolver]` + `#[query]`/`#[mutation]` | `@Resolver()` + `@Query()`/`@Mutation()` |
| `#[field]` | `@ResolveField()` |
| `#[dataloader]` | DataLoader |
| `#[cron_job]` | `@Cron()` |
| `#[processor]` | `@Processor()` |
| `#[event_handler]` | `@OnEvent()` / event-emitter |
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
| `nestrs-events` | `@nestjs/event-emitter` |
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

## Community & contributing

NestRS is young, and early contributors shape what it becomes — you don't have to
write Rust to help.

- 💬 **Ask a question, propose an idea, or just say hi** in [Discussions](https://github.com/NestRS/NestRS/discussions).
- 🐛 **Report a bug or request a feature** through [issues](https://github.com/NestRS/NestRS/issues/new/choose).
- 🌱 **Pick up a** [`good first issue`](https://github.com/NestRS/NestRS/labels/good%20first%20issue) — [CONTRIBUTING.md](CONTRIBUTING.md) is the short path from idea to merged PR.
- 🗺️ **See where it's heading** in the [roadmap](ROADMAP.md).
- 🔒 **Found a vulnerability?** Follow [SECURITY.md](SECURITY.md) — please don't open a public issue for it.

If NestRS resonates, a ⭐ helps others find it and tells us the direction is worth
pushing.

## License

MIT — see [LICENSE](LICENSE).

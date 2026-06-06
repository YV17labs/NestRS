<p align="center">
  <img src="assets/wordmark.svg" alt="NestRS" width="220">
</p>

<p align="center">
  <strong>Scalable Rust backend apps with native performance.</strong>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/built%20with-Rust-CE412B?logo=rust&logoColor=white" alt="Built with Rust">
  <img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT License">
  <img src="https://img.shields.io/badge/status-alpha-orange" alt="Status: alpha">
  <img src="https://img.shields.io/badge/PRs-welcome-brightgreen" alt="PRs welcome">
</p>

<p align="center">
  <a href="https://nestrs.dev"><strong>nestrs.dev</strong></a> — full documentation, tutorials and API reference.
</p>

> [!NOTE]
> **Alpha — under active development.** The API still shifts and rough edges
> remain, so it is not production-ready yet. Stars and early feedback are very
> welcome.

## Why NestRS

- ⚡ **Rust-native speed.** ~25× the throughput of an equivalent Node service on
  the same CPU budget (~13× per core), with a sub-millisecond p99 — built on the
  same hyper/tokio core as the fastest Rust web frameworks, with no GC pauses and
  tail latencies that stay flat under load. [See the benchmark.](#benchmark)
- 🪶 **An order of magnitude less memory.** ~4 MB idle and ~6 MB under load, versus
  ~80–120 MB for the same Node service — roughly 18–20× lighter, for smaller
  instances, higher density, and a lighter cloud bill.
- 🚀 **Boots in milliseconds.** A single static native binary with no runtime to
  warm up — friendly to autoscaling and cold starts.
- 🧩 **Declarative by design.** `#[module]`, `#[controller]`, `#[injectable]`,
  `#[resolver]`, `#[processor]` — features are wired with attribute macros, not
  hand-written boilerplate.
- 🛡️ **Verified before it serves.** The DI graph is wired by macros and checked at
  boot — a module can inject only what its imports reach (a compile-time
  encapsulation boundary a runtime `exports` list can't enforce), with no
  reflection and no runtime surprises.
- 🔐 **Security & transactions, transparent.** A service queries through `Repo`
  against an ambient, request-scoped data context — so every read is filtered to
  the caller's permissions and a mutating request runs in a transaction, with no
  hand-written authorization filter or transaction code.
- 📦 **Batteries included.** HTTP, GraphQL, OpenAPI, MCP, Redis-backed queues,
  scheduling, an event bus, CASL-style authorization, health probes,
  OpenTelemetry and an in-process test harness — each an opt-in crate, so you
  compile only what you import.

## Hello world

The umbrella `nestrs` crate re-exports the surface behind Cargo features; one
`use nest_rs::prelude::*;` brings in the everyday decorators and types.

```toml
# Cargo.toml
nest-rs = { version = "0", features = ["http"] }
```

(The published name is `nest-rs` on crates.io; in code, `use nest_rs::prelude::*;` still
works — the library exports under the `nestrs` name.)

```rust
// src/main.rs
use nest_rs::prelude::*;

// --- src/hello/service.rs ---
#[injectable]
#[derive(Default)]
struct HelloService;

impl HelloService {
    fn greeting(&self) -> &'static str { "Hello World" }
}

// --- src/hello/controller.rs ---
#[controller(path = "/")]
struct HelloController { #[inject] svc: std::sync::Arc<HelloService> }

#[routes]
impl HelloController {
    #[get("/")]
    async fn hello(&self) -> &'static str { self.svc.greeting() }
}

// --- src/hello/module.rs ---
#[module(imports = [HttpModule::for_root(None)], providers = [HelloService, HelloController])]
struct AppModule;

// --- src/main.rs ---
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    App::builder().module::<AppModule>().build().await?.run().await
}
```

For this minimal quickstart everything fits in a single `main.rs`; the
file headers above show where each piece lives once the app grows beyond
"Hello World" — one folder per feature, one file per role.

Prefer per-crate imports (`use nest_rs_http::controller;`) when you want to
see exactly which surface you reach for — both spellings resolve to the same
items.

## Pluggable layers

Every concern in NestRS is split into an **abstractions crate** (the trait
the framework calls) and a **first-party integration** (the default
implementation). Swap any layer by writing a sibling crate against the
public trait — no fork required.

| Abstraction | First-party implementation |
| ----------- | -------------------------- |
| `nest-rs-database` (`Executor`, `Repo`) | `nest-rs-seaorm` (SeaORM) |
| `nest-rs-queue` (`QueueBackend`) | `nest-rs-redis` (Redis via apalis) |
| `nest-rs-throttler` (`ThrottlerStore`) | bundled in-memory store |
| `nest-rs-config` (`ConfigSource`) | bundled env-based source |
| `nest-rs-authn` (`Strategy`) | JWT + OAuth2 strategies |

Each crate's `README.md` under [`crates/`](crates/) is the source of truth
for the contract its extension point exposes.

## Benchmark

The same "Hello World" HTTP service — a provider, a controller, a module —
implemented once in NestRS and once in NestJS, under an identical `wrk` load
(`GET /`, plaintext, keep-alive). On the same CPU budget NestRS served **~25×
more requests** while using **~20× less memory**.

| Metric — `GET /` plaintext      | NestRS (Rust)  | NestJS (Node 20) | Ratio  |
| ------------------------------- | -------------- | ---------------- | ------ |
| Throughput (2 cores, defaults)  | ~463k req/s    | ~18k req/s       | ~25×   |
| Throughput (1 core, per-core)   | ~231k req/s    | ~17k req/s       | ~13×   |
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
magnitude, and reproduce them with the <code>hello</code> example.</sub>

## Documentation

Everything user-facing lives at **[nestrs.dev](https://nestrs.dev)**:

- **[Getting started](https://nestrs.dev/getting-started/)** — install the toolchain and run your first endpoint.
- **[Tutorial](https://nestrs.dev/tutorial/)** — build a feature end-to-end.
- **[Fundamentals](https://nestrs.dev/fundamentals/)** — modules, providers, the DI graph, lifecycle.
- **HTTP, GraphQL, WebSockets, Database, Security, Queue, Schedule, Events, MCP, Health, Throttler, Server-Timing, OpenTelemetry, Configuration, Testing** — one section per capability crate.

This README is the contributor's entry point. For day-to-day usage, follow the docs.

## Contributing

Anyone who can clone the repo can iterate on the framework — the dev container
brings up Rust, Postgres and Redis in one step.

### Get the dev container running

1. Install [Docker](https://docs.docker.com/get-docker/) and the VS Code
   [Dev Containers](https://marketplace.visualstudio.com/items?itemName=ms-vscode-remote.remote-containers)
   extension.
2. Open the repo in VS Code and accept **Reopen in Container**.
3. `just dev` — the bare `hello` baseline on `http://localhost:3000`.

The container provisions the Rust toolchain and dev tooling (`just`, `bacon`,
`cargo-nextest`, …), and brings up **Postgres** and **Redis** beside it with
`NESTRS_DATABASE__URL` / `NESTRS_QUEUE__URL` already pointed at them. `just dev`
runs under `bacon` — every save triggers an incremental rebuild and a restart.

> Prefer a local toolchain? See [Getting started → On your own machine](https://nestrs.dev/getting-started/#on-your-own-machine).

### Project layout

A Cargo workspace with three kinds of member.

```
nestrs/
├─ apps/               applications — one runnable binary each
│  ├─ chat/            real-time WebSocket gateway
│  ├─ hello/           minimal HTTP baseline
│  ├─ mcp/             Model Context Protocol server
│  ├─ platform-api/    REST + GraphQL + OpenAPI, persisted & authorized
│  ├─ platform-auth/   OAuth2 / JWT token issuer
│  └─ platform-worker/ background jobs & scheduling (headless)
├─ crates/
│  ├─ features/        product features — port + adapters (users, authn, authz, …)
│  ├─ migrations/      shared-database SeaORM migrations (CLI)
│  ├─ seed/            shared-database demo data (CLI)
│  ├─ nest-rs-core/     IoC container, modules, DI, bootstrap
│  ├─ nest-rs-http/     REST controllers & routing
│  └─ …                one framework crate per capability
└─ docs/               the nestrs.dev site (Astro Starlight)
```

- **`apps/<name>/`** — `main.rs` + `module.rs` listing the edge modules the binary serves. One repository, several independently shippable services.
- **`crates/nestrs-*/`** — the framework: generic, product-agnostic building blocks.
- **`crates/features/`** — the product's vertical slices (entity, service, policy, per-transport adapters). Apps import the edges they serve; the same feature code backs every binary.

Adding an app means adding a directory under `apps/`; a new feature means a folder under `crates/features/src/`; a new framework capability means a `nestrs-*` crate. The workspace picks all three up automatically (`members = ["crates/*", "apps/*"]`).

### Commands

Run `just` with no arguments to list every recipe.

| Command | What it does |
|---------|--------------|
| `just dev <app>` | Run an app in watch mode (rebuild + restart on change), e.g. `just dev platform-api` |
| `just run <app>` | Run an app in release mode, e.g. `just run platform-api` |
| `just build` | Build release binaries for every app in the workspace |
| `just test` | Run unit + integration tests (no DB) |
| `just test-e2e` | Run e2e tests (Postgres required) |
| `just test-cov` | Run coverage on the full suite |
| `just lint` | Clippy (strict) + format check |
| `just fmt` | Apply rustfmt |
| `just check` | Fast type-check (no codegen) |
| `just db <verb>` | Manage the shared database: `up`, `down`, `fresh`, `status`, `seed`, `reset` |

`build`, `test`, `test-cov`, `lint`, `fmt` and `check` always operate on the
whole workspace; `dev` and `run` take an app name (default `hello`); `just db`
(run bare to list the verbs) manages the shared Postgres schema and seed data.

### Example apps

Each app is a different *kind* of binary, there to show that several can share
one workspace and the same building blocks. `platform-auth` and `platform-api`
together demonstrate the split-by-responsibility pattern — a dedicated token
issuer and a pure resource server that trust the same self-contained JWT and
share the `features` crate (`identity` module), never calling each other.

| App | Kind | Port |
|-----|------|------|
| `hello` | Minimal HTTP baseline | 3000 |
| `platform-auth` | OAuth2 / JWT token issuer | 3001 |
| `platform-api` | REST + GraphQL + OpenAPI, persisted & authorized | 3002 |
| `mcp` | Model Context Protocol server | 3003 |
| `chat` | Real-time WebSocket gateway | 3004 |
| `platform-worker` | Background jobs & scheduling (headless) | — |

`platform-api` and `platform-auth` need Postgres; `platform-worker` needs Redis
— run `just db up` once first (or `just db reset` to also load demo users). The
bare `hello`, `mcp` and `chat` examples need neither.

The richest reference is `platform-api`. Read it before inventing a second
pattern — copy it to start a new feature; see [`CLAUDE.md`](CLAUDE.md) for the
rules a contributor (human or LLM) is expected to follow.

### Docker

A multi-stage [`Dockerfile`](Dockerfile) at the repo root builds **every
workspace binary** into a single image. Which one runs is chosen at
`docker run` time.

```bash
docker build -t nestrs .
docker run --rm -p 3000:3000 nestrs                              # default `hello`
docker run --rm -p 3002:3002 nestrs /usr/local/bin/platform-api  # any other binary
docker run --rm nestrs /usr/local/bin/migrate up                 # apply migrations
```

Runtime image is `gcr.io/distroless/cc-debian13:nonroot` — no shell, no package
manager, runs as UID 65532. `cargo-chef` cooks dependencies in a cacheable
layer. Adding a new app under `apps/` requires no Dockerfile change.

## Community & contributing

NestRS is young, and early contributors shape what it becomes — you don't have
to write Rust to help.

- 💬 **Ask a question, propose an idea, or just say hi** in [Discussions](https://github.com/NestRS/NestRS/discussions).
- 🐛 **Report a bug or request a feature** through [issues](https://github.com/NestRS/NestRS/issues/new/choose).
- 🌱 **Pick up a** [`good first issue`](https://github.com/NestRS/NestRS/labels/good%20first%20issue) — [CONTRIBUTING.md](CONTRIBUTING.md) is the short path from idea to merged PR.
- 🗺️ **See where it's heading** in the [roadmap](ROADMAP.md).
- 🔒 **Found a vulnerability?** Follow [SECURITY.md](SECURITY.md) — please don't open a public issue for it.

If NestRS resonates, a ⭐ helps others find it and tells us the direction is worth
pushing.

## License

MIT — see [LICENSE](LICENSE).

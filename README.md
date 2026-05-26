<p align="center">
  <img src="assets/logo.svg" alt="nestrs logo" width="160" height="160">
</p>

# NestRS

An opinionated Rust framework that leans on procedural macros to keep
application code declarative. NestJS-inspired on the surface, Rust-native
underneath.

Applications live under `apps/`, reusable building blocks under `crates/`.

## Prerequisites

- Rust toolchain (1.75 or newer): https://rustup.rs

## One-time setup

Install the dev tooling:

```bash
cargo install --locked just bacon cargo-nextest cargo-llvm-cov
rustup component add llvm-tools-preview
```

| Tool | Purpose |
|------|---------|
| [`just`](https://github.com/casey/just) | Task runner — equivalent of npm scripts |
| [`bacon`](https://dystroy.org/bacon/) | Watcher — rebuilds and restarts on save |
| [`cargo-nextest`](https://nexte.st) | Parallel test runner, noticeably faster than `cargo test` |
| [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) | Source-based test coverage (uses LLVM, plays well with nextest) |

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
workspace; `dev` and `run` take an app name (default `api`).

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

## Applications

### `api` — HTTP + GraphQL (port 3002)

Started with `just dev api`. Listens on `http://0.0.0.0:3002`:

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
fields, and `#[field]` adds a field resolver (NestJS's `@ResolveField`) to an
object type — it takes the resolved object as `parent: &T` and reaches services
through the resolver's `#[inject]` fields. The schema composes itself from every
resolver in the binary (no central list) and is committed as SDL at
[`apps/api/schema.graphql`](apps/api/schema.graphql), so API changes surface in
diffs. A dev run of the server rewrites that file on boot (`GraphqlModule`'s
`emit_sdl` is `true` under `debug_assertions`, `false` in a release build); commit
the result after touching a resolver.

The REST surface documents itself the same way: import `OpenApiModule` and the
OpenAPI document composes from every `#[controller]` in the binary — verbs and
paths from the route table, request/response schemas from each `Json<T>` payload
(DTOs derive `schemars::JsonSchema`, the same trait MCP uses), grouped by
controller. `#[api(summary = "...", tags("..."))]` beside a verb enriches an
operation (NestJS's `@ApiOperation`/`@ApiTags`). Swagger UI is bundled and served
offline at `/api` (the document at `/api-json`), matching NestJS's default paths.

It also exercises the request pipeline: `POST /users` is protected by an
`#[injectable]` `ApiKeyGuard` bound with `#[use_guards]` (send an `x-api-key`
header), which attaches a `Caller` the handler reads back via `Ctx<Caller>`. And
a `#[cron_job]` (`UserCountReport`), ticked by the `Scheduler` transport, logs
the user count on an interval.

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

## License

MIT — see [LICENSE](LICENSE).

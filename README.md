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
| `just dev` | Run the API in watch mode (rebuild + restart on change) |
| `just run` | Run the API in release mode |
| `just build` | Build the release binary |
| `just test` | Run the full test suite |
| `just cov` | Test coverage summary (per-file %) |
| `just lint` | Clippy (strict) + format check |
| `just fmt` | Apply rustfmt |
| `just check` | Fast type-check (no codegen) |

## Docker

A multi-stage [`Dockerfile`](Dockerfile) at the repo root builds **every
workspace binary** into a single image. Which one runs is chosen at `docker
run` time:

```bash
docker build -t nestrs .

# Run the default app (api)
docker run --rm -p 3000:3000 nestrs

# Run a different app by overriding the entrypoint
docker run --rm -p 3000:3000 nestrs /usr/local/bin/<other-app>
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

## Once the API is running

It listens on `http://0.0.0.0:3000`:

| Endpoint | Purpose |
|----------|---------|
| `POST /graphql` | GraphQL endpoint |
| `GET  /graphql` | GraphQL playground |
| `GET  /health/live` | Kubernetes liveness probe |
| `GET  /health/ready` | Kubernetes readiness probe |
| `GET  /health/startup` | Kubernetes startup probe |

## License

MIT — see [LICENSE](LICENSE).

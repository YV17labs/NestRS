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

> [!NOTE]
> **Alpha — under active development.** The API still shifts and rough edges
> remain, so it is not production-ready yet. Stars and early feedback are very
> welcome.

## Documentation

**Using NestRS?** Head to **[nestrs.dev](https://nestrs.dev)** — getting started,
tutorial, [why NestRS](https://nestrs.dev/why/), benchmarks, and one section per
capability crate.

**Contributing to the framework?** This README is your entry point. For design
rules and conventions, read [`CLAUDE.md`](CLAUDE.md) and
[`CONTRIBUTING.md`](CONTRIBUTING.md).

## Contributing

Anyone who can clone the repo can iterate on the framework — the dev container
brings up Rust, Postgres and Redis in one step.

### Get the dev container running

1. Install [Docker](https://docs.docker.com/get-docker/) and the VS Code
   [Dev Containers](https://marketplace.visualstudio.com/items?itemName=ms-vscode-remote.remote-containers)
   extension.
2. Open the repo in VS Code and accept **Reopen in Container**.
3. `nestrs run dev api` — the main Publish API on `http://localhost:3002` (run `nestrs run db up` first).

The container provisions the Rust toolchain and dev tooling (`just`, `bacon`,
`cargo-nextest`, …), and brings up **Postgres** and **Redis** beside it with
`NESTRS_DATABASE__URL` / `NESTRS_QUEUE__URL` already pointed at them. `nestrs run dev`
runs under `bacon` — every save triggers an incremental rebuild and a restart.

> Prefer a local toolchain? See [Getting started → On your own machine](https://nestrs.dev/getting-started/#on-your-own-machine).

### Project layout

A Cargo workspace with three kinds of member.

```
nestrs/
├─ apps/               applications — one runnable binary each (the Publish workspace)
│  ├─ auth/   OAuth2 / JWT token issuer
│  ├─ api/    REST + GraphQL + OpenAPI, persisted & authorized
│  ├─ assistant/  Model Context Protocol server
│  ├─ live/   real-time WebSocket gateway
│  └─ worker/ background jobs & scheduling (headless)
├─ crates/
│  ├─ features/        product features — port + adapters (users, posts, authn, …)
│  ├─ migrations/      shared-database SeaORM migrations (CLI)
│  ├─ seed/            shared-database demo data (CLI)
│  ├─ nest-rs-core/     IoC container, modules, DI, bootstrap
│  ├─ nest-rs-http/     REST controllers & routing
│  └─ …                one framework crate per capability
└─ docs/               the nestrs.dev site (Astro Starlight)
```

Simple **hello** and **blog** layouts are CLI-scaffolded only — see [Getting started](https://nestrs.dev/getting-started/) and the [tutorial](https://nestrs.dev/tutorial/). They are not checked into this repo.

- **`apps/<name>/`** — `main.rs` + `module.rs` listing the edge modules the binary serves. One repository, several independently shippable services.
- **`crates/nestrs-*/`** — the framework: generic, product-agnostic building blocks.
- **`crates/features/`** — the product's vertical slices (entity, service, policy, per-transport adapters). Apps import the edges they serve; the same feature code backs every binary.

Adding an app means adding a directory under `apps/`; a new feature means a folder under `crates/features/src/`; a new framework capability means a `nestrs-*` crate. The workspace picks all three up automatically (`members = ["crates/*", "apps/*"]`).

### Commands

Run `nestrs run` with no arguments to list every recipe.

| Command | What it does |
|---------|--------------|
| `nestrs run dev <app>` | Run an app in watch mode (rebuild + restart on change), e.g. `nestrs run dev api` |
| `nestrs run start <app>` | Run an app in release mode, e.g. `nestrs run start api` |
| `nestrs run build <app>` | Build one app in release (default `api`), e.g. `nestrs run build live` |
| `nestrs run build --all` | Build release binaries for every app in the workspace |
| `nestrs run test unit` | Run unit + integration + doctests (no DB) |
| `nestrs run test e2e` | Run e2e tests (Postgres required) |
| `nestrs run test cov` | Run coverage on the full suite |
| `nestrs run test doc` | Run doctests only (`///` examples) |
| `nestrs run lint` | Clippy (strict) + format check |
| `nestrs run fmt` | Apply rustfmt |
| `nestrs run check` | Fast type-check (no codegen) |
| `nestrs run db <verb>` | Manage the shared database: `up`, `down`, `fresh`, `status`, `seed`, `reset` |

`build --all`, `test` (with `e2e` / `cov` / `doc`), `lint`, `fmt` and `check`
operate on the whole workspace; `dev`, `start`, and `build` take an app name (default `api`);
`nestrs run db` (run bare to list the verbs) manages the shared Postgres schema and seed data.

### The Publish workspace

This repo ships **Publish** — a fictional multi-tenant publishing platform
told through six apps that share `crates/features/` and never RPC each other.
Full map: [nestrs.dev/publish](https://nestrs.dev/publish/).

| App | Kind | Port |
|-----|------|------|
| `auth` | OAuth2 / JWT token issuer | 3001 |
| `api` | REST + GraphQL + OpenAPI, persisted & authorized | 3002 |
| `assistant` | Model Context Protocol server | 3003 |
| `live` | Real-time WebSocket gateway | 3004 |
| `worker` | Background jobs & scheduling (headless) | — |

`api` and `auth` need Postgres; `worker` needs Redis
— run `nestrs run db up` once first (or `nestrs run db reset` to also load demo users).
`assistant` and `live` need neither.

The richest reference is `api`. Read it before inventing a second
pattern — copy it to start a new feature; see [`CLAUDE.md`](CLAUDE.md) for the
rules a contributor (human or LLM) is expected to follow.

### Docker

A multi-stage [`Dockerfile`](Dockerfile) at the repo root builds **every
workspace binary** into a single image. Which one runs is chosen at
`docker run` time.

```bash
docker build -t nestrs .
docker run --rm -p 3002:3002 nestrs                              # default `api`
docker run --rm -p 3001:3001 nestrs /usr/local/bin/auth  # any other binary
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

<p align="center">
  <img src="assets/wordmark.svg" alt="NestRS" width="220">
</p>

<p align="center">
  <strong>Scalable Rust backend apps with native performance.</strong>
</p>

<p align="center">
  <a href="https://github.com/YV17labs/NestRS/stargazers"><img src="https://img.shields.io/github/stars/YV17labs/NestRS?style=flat&logo=github&color=555" alt="GitHub stars"></a>
  <img src="https://img.shields.io/badge/built%20with-Rust-CE412B?logo=rust&logoColor=white" alt="Built with Rust">
  <img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT License">
  <img src="https://img.shields.io/badge/PRs-welcome-brightgreen" alt="PRs welcome">
</p>

## What it is

You write business logic; the framework carries the rest — authn, authz,
row-level tenant filtering, per-field masking, transactions, discovery, and
lifecycle. Those concerns are transparent to your code and verified at boot,
not left to review discipline.

A full authenticated, tenant-scoped, transactional, field-masked CRUD resource
is an empty `impl` block:

```rust
#[controller(path = "/orgs")]
#[use_guards(AuthGuard, AuthzGuard)]
pub struct OrgsController {
    #[inject]
    svc: Arc<OrgsService>,
}

#[crud(service = svc, entity = OrgEntity, output = Org,
       create = CreateOrg, update = UpdateOrg)]
impl OrgsController {}
```

The differentiator is **structural multi-tenant isolation you cannot forget**:
row-level filtering, response masking, and transactions become non-optional the
moment the security modules are imported. A feature opts *out* by not importing
them. In NestJS, Spring, Rails, axum, or Loco, tenant filtering is discipline —
a scope you remember, a middleware you apply, a review comment. In NestRS it is
structural: the data layer applies it from the caller's ability, and an
operation with no declared access posture is refused — at compile time on
GraphQL, at boot on HTTP.

And it stays lean. On the same hello-world service under identical `wrk` load,
NestRS serves **~463k req/s** to NestJS 11's ~18k — **~25×** — in **4–6 MB** of
RAM against ~80–120 MB. Our own measurements of the demo API — the full JWT +
authz + row-level + masking pipeline, Postgres included — still land around
**23k req/s**, **p99 < 4.5 ms**, in **~32 MB** of resident memory; the binary
ships at 11–20 MB and boots in tens of milliseconds. (Measured in a Linux
Docker container capped at 4 cores and 8 GB, with the load generator competing
for the same cores — a setup that understates the numbers; dedicated hardware
only pushes them up.)

Try it on your machine:

```bash
cargo install --locked nest-rs-cli
nestrs new hello --standalone && cd hello
nestrs run dev   # → Hello World on :3000
```

→ [Why not axum?](https://nestrs.dev/why-not-axum/) ·
[Coming from NestJS](https://nestrs.dev/coming-from-nestjs/) ·
[Why NestRS](https://nestrs.dev/why/)

## Documentation

**Using NestRS?** Head to **[nestrs.dev](https://nestrs.dev)** — getting started,
tutorial, [why NestRS](https://nestrs.dev/why/), the
[axum comparison](https://nestrs.dev/why-not-axum/), and one section per
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
3. `cd demo && nestrs run dev api` — the main Publish API on `http://localhost:3002` (run `nestrs run db up` first).

The container provisions the Rust toolchain and dev tooling (`just`, `bacon`,
`cargo-nextest`, …), and brings up **Postgres** and **Redis** beside it with
`NESTRS_DATABASE__URL` / `NESTRS_QUEUE__URL` already pointed at them. `nestrs run dev`
runs under `bacon` — every save triggers an incremental rebuild and a restart.
The runnable apps live in their own workspace under [`demo/`](demo/) — `cd demo`
first; that directory is where `nestrs run`, the `.env` cascade, and the
database/test recipes resolve.

> Prefer a local toolchain? See [Getting started → Scaffold and start](https://nestrs.dev/getting-started/#scaffold-and-start).

### Project layout

**Two Cargo workspaces**, split along the framework/product line.

```
nestrs/
├─ crates/              the framework — one nest-rs-* crate per capability
│  ├─ nest-rs-core/      IoC container, modules, DI, bootstrap
│  ├─ nest-rs-http/      REST controllers & routing
│  └─ …                 (members = ["crates/*"])
├─ docs/                the nestrs.dev site (Astro Starlight)
└─ demo/                the product — its own workspace, consumes the framework
   ├─ apps/              one runnable binary each (the Publish workspace)
   │  ├─ auth/   OAuth2 / JWT token issuer
   │  ├─ api/    REST + GraphQL + OpenAPI, persisted & authorized
   │  ├─ assistant/  Model Context Protocol server
   │  ├─ live/   real-time WebSocket gateway
   │  └─ worker/ background jobs & scheduling (headless)
   ├─ crates/
   │  ├─ features/       product features — port + adapters (users, posts, authn, …)
   │  ├─ migrations/     shared-database SeaORM migrations (CLI)
   │  └─ seed/           shared-database demo data (CLI)
   ├─ Justfile, db.just, test.just, .env*, Dockerfile
   └─ (members = ["apps/*", "crates/*"])
```

The **`demo/`** workspace references the framework by relative path
(`nest-rs-* = { path = "../crates/nest-rs-*" }`), so it builds against the
live framework source. You `cd demo` and drive it as if it were the app's own
repository — see [`demo/README.md`](demo/README.md) for running the apps, the
command table, the Publish map, and Docker.

- **`crates/nest-rs-*/`** — the framework: generic, product-agnostic building blocks.
- **`demo/apps/<name>/`** — `main.rs` + `module.rs` listing the edge modules the binary serves.
- **`demo/crates/features/`** — the product's vertical slices; apps import the edges they serve.

Adding an app means a directory under `demo/apps/`; a new feature means a folder
under `demo/crates/features/src/`; a new framework capability means a `nest-rs-*`
crate under `crates/`. Simple **hello**/**blog** layouts are CLI-scaffolded only
— see [Getting started](https://nestrs.dev/getting-started/) and the
[tutorial](https://nestrs.dev/tutorial/); they are not checked into this repo.

### Running the apps

Everything runnable lives in [`demo/`](demo/) — `cd demo` first, then
`nestrs run` (no args lists every recipe). The full command table, the Publish
app map, and the Docker build are documented in
[`demo/README.md`](demo/README.md).

## Community & contributing

NestRS is young, and early contributors shape what it becomes — you don't have
to write Rust to help.

- 💬 **Ask a question, propose an idea, or just say hi** in [Discussions](https://github.com/YV17labs/NestRS/discussions).
- 🐛 **Report a bug or request a feature** through [issues](https://github.com/YV17labs/NestRS/issues/new/choose).
- 🌱 **Pick up a** [`good first issue`](https://github.com/YV17labs/NestRS/labels/good%20first%20issue) — [CONTRIBUTING.md](CONTRIBUTING.md) is the short path from idea to merged PR.
- 🗺️ **See where it's heading** in the [roadmap](ROADMAP.md).
- 🔒 **Found a vulnerability?** Follow [SECURITY.md](SECURITY.md) — please don't open a public issue for it.

If NestRS resonates, a ⭐ helps others find it and tells us the direction is worth
pushing.

## License

MIT — see [LICENSE](LICENSE).

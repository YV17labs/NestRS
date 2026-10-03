---
paths:
  - "demo/apps/**/*.rs"
  - "demo/apps/**/*.toml"
  - "demo/**/*.just"
  - "demo/Justfile"
  - "demo/Dockerfile"
---

# Apps — pure composition

**An app's `src/` is `main.rs`, `module.rs` and `lib.rs`.**

- **`module.rs`** is the canonical composition: one `<App>Module` listing the
  edges the app serves.
- **`lib.rs`** declares `mod module;` and re-exports `<App>Module` — and the
  types its e2e suite configures it with — so the suite boots the composition
  the binary runs. No logic.
- **`main.rs`** builds the app — `App::builder().module::<AppModule>()` — plus
  the imperative global seams (`use_*_global`, `edges.md`, *Request layers*)
  and, where the app exports, `OpenTelemetry::init` (`container.md`, the one
  `for_root` exception). Declaring the transport-wide pool is composition, not
  business logic.

No `services/`, no `examples/`, and never an edge folder under the app's
`src/`: an adapter there could only be named for the app, and the app's name
stops at `<App>Module` (`architecture.md`).

## The apps

| App | Serves |
|---|---|
| `api` | REST, GraphQL, the database and authz — **the reference app** |
| `live` | WebSockets |
| `auth` | the token issuer (it signs; `api` only verifies) |
| `assistant` | MCP |
| `worker` | the queue, and the one-replica notifications purge |

The starter layout is scaffolded by the CLI and documented on the site, never
hosted here.

## App-local features are the exception

A feature folder under `apps/<x>/` is allowed only when **this app's exposure
decides something the feature cannot generalize** — glue over several features,
or a deployment-specific route. It may then flatten (handler, `service.rs`,
`module.rs` at the folder root, no port/adapter split). Business logic another
app could serve belongs in `demo/crates/features/` whatever its size; code
being small or single-transport today is never the reason. The demo holds no
app-local feature.

## Several deployable apps

Splitting apps by responsibility is a goal, under two conditions: **code is
shared through crates**, never copied — product logic lives in
`demo/crates/features/` — and **coupling stays loose**: a self-contained token
and a shared database, never RPC (`CLAUDE.md`, hard "no").

## Running the product

`cd demo` and drive it as its own repo; `nestrs run <recipe>` forwards to
`just`, whose recipes are `demo/Justfile`, `db.just` and `test.just`. The
`.env` cascade, the `Dockerfile` — built with the parent directory as context,
so it reaches `../crates` — and a separate `Cargo.lock` and `target/` all live
under `demo/`. The root workspace's `Justfile` holds the checks CI runs (`just
pre-commit`, `just ci`) and nothing else; the framework is otherwise driven with
bare `cargo`.

## Transports and output

An app activates a transport or a stack by importing its module
(`HttpModule::for_root(…)`, `RedisModule::for_root(…)`, `OpenApiModule`, …),
never through a `.transport(…)` call (`edges.md`, *Surface decisions*). A
module's settings are pinned through its `for_root` or set through its
variables (`architecture.md`, *Configuration*).

**A deployed app exports OTLP**: `main` holds the `OpenTelemetry::init` guard
and the root imports `OpenTelemetryModule`, which refuses to register without
it. The console format follows the build profile (`nest_rs_core::logging`).

**A GraphQL app commits its SDL** (`apps/<app>/schema.graphql`), regenerated as
a side effect of the dev run — there is no standalone generator.

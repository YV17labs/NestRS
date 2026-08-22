//! Files shared by standalone and workspace scaffolds (env cascade, gitignore, …).

pub const RUST_TOOLCHAIN: &str = r#"[toolchain]
channel = "1.97"
"#;

/// `db.just` — shipped in every project so the database verbs are present from
/// day one, whether or not the project has a database yet. Recipes follow the
/// nestrs convention: a `migrations` crate (the `migrate` bin) and a `seed` crate.
/// They start working once you add those — see the database docs.
pub const DB_JUSTFILE: &str = r#"# Database lifecycle, exposed as `nestrs run db <verb>` (see `mod db` in the
# Justfile). Recipes assume the nestrs `migrations` + `seed` crates.

# Bare `nestrs run db` lists these instead of running the first recipe.
_default:
    @just --list db

# Apply all pending migrations.
up:
    cargo run -p migrations --bin migrate -- up

# Roll back the last applied migration (`nestrs run db down 3` reverts the last 3).
down n='1':
    cargo run -p migrations --bin migrate -- down {{n}}

# Drop every table and re-apply all migrations from scratch.
fresh:
    cargo run -p migrations --bin migrate -- fresh

# Show which migrations are applied vs. pending.
status:
    cargo run -p migrations --bin migrate -- status

# Seed demo data (idempotent).
seed:
    cargo run -p seed --bin seed

# Clean slate: drop, re-migrate, then reseed.
reset: fresh seed
"#;

/// `compose.yml` — Postgres + Redis for local development, shipped so a
/// DB-backed feature works the moment you add one: `docker compose up -d`,
/// then `nestrs run db up`. The committed `.env` points at these services on
/// `localhost`. Delete it if your project never touches a database or a queue.
pub const COMPOSE: &str = r#"# Local development services. Start them with:
#
#   docker compose up -d
#
# The committed `.env` points {{env_prefix}}_DATABASE__URL / {{env_prefix}}_QUEUE__URL at these
# on localhost. `nestrs run db up` then applies your migrations.

services:
  postgres:
    image: postgres:16
    environment:
      POSTGRES_USER: {{kebab}}
      POSTGRES_PASSWORD: {{kebab}}
      POSTGRES_DB: {{kebab}}
    ports:
      - "5432:5432"
    volumes:
      - pgdata:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U {{kebab}}"]
      interval: 5s
      timeout: 3s
      retries: 10

  redis:
    image: redis:7
    ports:
      - "6379:6379"
    volumes:
      - redisdata:/data

volumes:
  pgdata:
  redisdata:
"#;

/// The line that puts the project's env prefix on every process `just` starts.
///
/// A template rather than a `format!` in the command module: everything the CLI
/// writes into a generated project lives here, which is also what keeps it under
/// the mechanical guards in `super::tests`.
pub const ENV_PREFIX_JUSTFILE: &str = r#"# Every framework variable carries this prefix ({{env_prefix}}_ENV, {{env_prefix}}_HTTP__PORT, …).
# It must be set on the process, so it lives here and in your deployment —
# never in `.env`, which is read too late to have chosen itself.
export {{env_prefix_var}} := "{{env_prefix}}"

"#;

/// Why the `dev` recipe sets `<PREFIX>_ENV` on the command line — the workspace
/// and standalone Justfiles differ in every other line of that recipe, but this
/// story is one story and drifts the moment it is told twice.
pub const DEV_RECIPE_NOTE: &str = r#"# `{{env_prefix}}_ENV` is set here rather than in `.env`: it selects the `.env`
# cascade, so it has to exist before any file is read. It also arms every
# development-only affordance (the `POST /auth/dev-token` route `nestrs g auth`
# writes), which is why absence has to mean "not development" everywhere else."#;

/// The same, baked into the image — overridable at `docker run` time, so one
/// image can still be run under another prefix.
pub const ENV_PREFIX_DOCKERFILE: &str = r#"
# Every framework variable carries this prefix. Override at `docker run`
# time to run the same image under another one.
ENV {{env_prefix_var}}={{env_prefix}}
"#;

pub const GITIGNORE: &str = r#"/target
**/*.rs.bk

# Coverage (cargo-llvm-cov)
*.profraw
*.profdata
/coverage

# Local secrets (see `.env.example`)
.env.local
.env.*.local

# Editor / OS
.idea/
*.swp
.DS_Store
"#;

pub const DOCKERIGNORE: &str = r#"target/
.git/
.env.local
.env.*.local
"#;

pub const ENV: &str = r#"# {{env_label}} — committed base config (`.env` cascade).
#
# Only overrides live here; omitted keys use in-code defaults. Real environment
# variables always win. Per-machine secrets go in `.env.local` (git-ignored);
# see `.env.example`.
#
# Precedence (highest first):
#   real env  >  pinned in `module.rs`  >  .env.<{{env_prefix}}_ENV>.local  >  .env.local
#   >  .env.<{{env_prefix}}_ENV>  >  .env

# HTTP server listen port (default: 3000).
{{env_prefix}}_HTTP__PORT=3000
"#;

/// Workspace root `.env` — the HTTP port's *default* lives in each app's
/// `module.rs`, but every `<PREFIX>_HTTP__*` key stays live over it.
pub const ENV_WORKSPACE: &str = r#"# {{env_label}} — committed base config (`.env` cascade).
#
# Each app's root `module.rs` sets its own HTTP defaults
# (`HttpConfig { port: …, ..Default::default() }`). Those are defaults, not a
# lock: any `{{env_prefix}}_HTTP__*` key set in the real environment still wins, field
# by field — so a deployment moves the port or turns on TLS without touching
# the code.
#
# Postgres + Redis as `compose.yml` exposes them on localhost. Start them with
# `docker compose up -d`, then `nestrs run db up`. An app only connects if it
# imports SeaOrmDatabaseModule / a queue module, so these are inert for a plain HTTP app.
{{env_prefix}}_DATABASE__URL=postgres://{{kebab}}:{{kebab}}@localhost:5432/{{kebab}}
{{env_prefix}}_QUEUE__URL=redis://localhost:6379
#
# Precedence (highest first):
#   real env  >  pinned in `module.rs`  >  .env.<{{env_prefix}}_ENV>.local  >  .env.local
#   >  .env.<{{env_prefix}}_ENV>  >  .env
"#;

pub const ENV_DEVELOPMENT: &str = r#"# {{env_label}} — development-only overrides. An unset {{env_prefix}}_ENV still loads
# this cascade, but arms no development-only affordance: those need it set, on the
# process, to development, dev or test — `nestrs run dev` does it. Setting it here
# would be too late and is refused at boot.
# Committed; layered on top of `.env`, below `.env.local` and the real environment.

# Verbose, human-readable logs while developing.
{{env_prefix}}_LOG=debug
{{env_prefix}}_LOG_FORMAT=text
{{env_prefix}}_LOG_SOURCE_LOCATION=true
"#;

/// The file that *instructs* a developer to write `.env.local`, so it is where
/// the exception belongs: the cascade skips `.env.local` under `<PREFIX>_ENV=test`
/// (hermetic by design). Without that line, a developer whose Postgres is not on
/// the default port edits `.env.local`, watches `nestrs run test e2e` fail to
/// connect, and has nothing pointing at the file being ignored.
pub const ENV_EXAMPLE: &str = r#"# Copy to `.env.local` for machine-specific or secret-shaped settings:
#
#   cp .env.example .env.local
#
# Tests are hermetic: under {{env_prefix}}_ENV=test the cascade skips `.env.local`, so a
# machine-specific test override (a different Postgres port, say) goes in
# `.env.test.local` — also git-ignored. See https://nestrs.dev/configuration/env-cascade/
#
# Uncomment when you add a database (https://nestrs.dev/configuration/).

# {{env_prefix}}_DATABASE__URL=postgres://user:pass@localhost:5432/{{kebab}}
# {{env_prefix}}_QUEUE__URL=redis://localhost:6379
"#;

/// The scaffolded smoke test. `{{smoke_use}}` / `{{smoke_module}}` name the
/// **narrowest** module that serves the greeting — the feature's HTTP module in
/// workspace mode, the crate's root module standalone (where they are the same
/// thing). In-process through `TestApp`, no live infra, so it belongs to the
/// `integration` suite.
///
/// Booting the app *root* here was the trap: the moment a resource is wired the
/// way `g resource` instructs, the root imports `SeaOrmDatabaseModule`, the
/// connection opens during `build()`, and the suite that `test.just` and
/// `/testing/integration-tests/` both define as the infrastructure-free one
/// fails with a 30 s pool timeout. Booting the feature module keeps the promise
/// no matter what the app grows into — and the emitted file says so, because
/// the developer who later adds a database is the one who has to know why this
/// suite boots a feature rather than the root.
pub const SMOKE: &str = r#"//! In-process smoke test — boots the feature's own module through `TestApp`,
//! no live infra, so it belongs to the `integration` suite and runs on every
//! `nestrs run test unit`. Tests needing a database, queue or object store go
//! next door in `tests/e2e/main.rs`.
//!
//! It deliberately boots the *feature* module rather than the app root: the app
//! root grows every transport and connection the app serves, and this suite
//! must stay infrastructure-free. Assert on the composed app in `tests/e2e/`.

use {{smoke_use}};
use nest_rs::testing::TestApp;

#[tokio::test]
async fn hello_endpoint_greets() {
    let app = TestApp::builder()
        .module::<{{smoke_module}}>()
        .build()
        .await
        .expect("{{smoke_module}} boots and mounts its routes");

    let resp = app.http().get("/").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("Hello World").await;
}
"#;

/// `AGENTS.md`, in two pieces: a layout header per shape, then the shared
/// conventions body ([`AGENTS_BODY`]).
///
/// Scaffolded because the conventions are **not inferable from the code**. A
/// tree of four files shows no rule for the fifth: which tier a new type
/// belongs to, what a provider that is not a service is called, where a second
/// service goes. A project without this file re-derives all of it — differently
/// — the first time it grows, and both a human and an agent read it as license.
/// Every rule below is the framework's own (`CLAUDE.md`, `features.md`),
/// restated for a project that does not have those files.
/// The title and the one paragraph both layouts open with. Split out because
/// a fix applied to one head would otherwise ship silently one-sided — nothing
/// compares the two.
pub const AGENTS_INTRO: &str = r#"# AGENTS.md — {{pascal}}

How this project is laid out and named. `nestrs new` wrote this file; it is
yours to edit. Read it before adding a file: the conventions below cannot be
inferred from the tree, and drifting from them is what turns a slice into a
folder nobody can navigate.

"#;

pub const AGENTS_STANDALONE_HEAD: &str = r#"## Layout — one crate

```
src/
  main.rs        boot only — App::builder().module::<{{module}}>()
  lib.rs         `mod` + `pub use`, no logic
  module.rs      the root DI module — composition
  service.rs     domain logic
  controller.rs  the HTTP edge, thin
tests/
  integration/   in-process, no live infrastructure
  e2e/           needs Postgres / Redis / object storage
```

A concern that grows past one service and one handler moves into its own
folder — `src/<feature>/{module,service,controller}.rs` — and `src/module.rs`
imports it. `nestrs g` needs a workspace, so features here are written by
hand; run `nestrs new <name>` beside this crate to grow into one once a second
binary needs the same logic.
"#;

pub const AGENTS_WORKSPACE_HEAD: &str = r#"## Layout — two homes, and the rule that divides them

```
apps/<app>/src/     main.rs + module.rs only — pure composition
crates/features/    the product's vertical slices, shared by every app
```

**`crates/features/` when any other app could reuse it; `apps/<app>/` only
when this app's exposure decides something the feature cannot generalize.**

A feature is a **port** plus one **adapter per transport**. The port sits at
the feature root; each adapter gets a sub-folder with its own `module.rs`, and
an app imports only the edges it actually serves.

```
crates/features/src/<feature>/
  module.rs entity.rs service.rs dto.rs error.rs   the port
  http/     module.rs controller.rs
  graphql/  module.rs resolver.rs
  ws/       module.rs gateway.rs
  queue/    module.rs processor.rs
  mcp/      module.rs tool.rs
```

`nestrs g feature|resource|http|graphql|ws|queue|schedule|mcp` writes that
shape and performs the two wiring edits a copy cannot carry — the `pub mod`
line in `crates/features/src/lib.rs` and the module entry in the serving app's
`module.rs`. Prefer it over hand-copying.

**The shape is invariant.** One transport, one adapter sub-folder, one
`<Feature><Edge>Module`, every time. Never invert it into a single top-level
edge folder that injects every domain service: that trades the module gate —
an app importing exactly the edges it serves — for an adapter no app can
subset. If a transport seems unable to host two features at one mount point,
that is a framework defect worth reporting, not a reason to flatten.

## Crates — a type, and a direction

Every crate has a type, and the type decides what it may depend on. An arrow
that points back up is a defect, not a trade-off. Cargo enforces most of this
for you: a crate that does not list another as a dependency cannot reach it at
all.

| Crate | Type | May depend on |
|---|---|---|
| `apps/<app>` | composition | everything |
| `crates/features` | feature | the framework, substrates |
| a substrate (`crates/<name>`) | util | third parties only — **never** the framework, never features |
| `crates/migrations`, `crates/seed` | tooling | binaries, outside the graph |

## Database — migrations and seed

`nestrs g migration <name>` writes the file, its `mod` line in
`crates/migrations/src/lib.rs`, and regenerates `migrator.rs` from that list —
the two registrations cannot drift, and neither is written by hand.

**The `DeriveIden` enum in a migration names the table, not the migration.**
`DeriveIden` snake-cases it straight into the DDL, so it must agree with the
entity's `#[sea_orm(table_name = "...")]` or the entity reads a table nothing
created. The scaffolded body creates a table with the house columns
(`created_at` / `updated_at` / `deleted_at`, matching what `#[expose(...,
soft_delete, timestamps)]` expects); swap it for an `alter_table` to change an
existing one. Every column you add goes in **twice** — once in the builder, once
as a variant of that enum.

`crates/seed/` runs on `nestrs run db seed`, and `nestrs run db reset` runs it
against a database that may already hold rows — so every insert there is
idempotent (find-or-create, or `ON CONFLICT DO NOTHING`).
"#;

/// `CLAUDE.md`, which is *only* a pointer at `AGENTS.md`.
///
/// Two files rather than one because no single name is read by everything:
/// `AGENTS.md` is the cross-tool convention, and Claude Code reads `CLAUDE.md`
/// alone. An import keeps one source of truth without a second copy to drift.
///
/// An import rather than `ln -s AGENTS.md CLAUDE.md`: a symlink needs
/// Administrator rights or Developer Mode on Windows, which a scaffold cannot
/// require. The note is an HTML comment — stripped before the file enters an
/// agent's context, so it costs no tokens and reads as intended by whoever
/// opens the file.
pub const CLAUDE_POINTER: &str = r#"@AGENTS.md

<!--
This project's conventions live in AGENTS.md, the format every coding agent
reads. Claude Code reads CLAUDE.md only, so this file imports it: write the
conventions in AGENTS.md and leave this one as the pointer. Instructions meant
for Claude Code alone belong below the import.
-->
"#;

/// The conventions, in two pieces.
///
/// The architecture model is `architecture.md` beside this file — one copy,
/// embedded here and symlinked into `.claude/rules/`, so the rules this repo
/// works under and the rules it ships are the same bytes.
///
/// **The real file is the build's, the symlink is `.claude/`'s**, and not the
/// reverse: a checkout with `core.symlinks=false` (Windows without Developer
/// Mode) materializes a link as a text file holding its target path, so an
/// inverted arrangement would embed that path into every scaffolded
/// `AGENTS.md` and compile clean. `.claude/` degrading there costs a session
/// its rules; the build silently shipping a filename does not degrade, it
/// lies. `cargo package` follows a symlink under the package root and archives
/// its bytes, so publishing does not decide this — the failure mode does.
///
/// The split point is the symlink, not the placeholders: `render` runs over
/// the whole document, so an embedded `{{key}}` would substitute fine. What it
/// cannot do is read as rules through the raw symlink, where a placeholder
/// stays literal — so everything per-project lives in the half below.
/// `static`, not `const`: a `const` is re-materialized at every use site, and
/// this one embeds ~9 KB from two modules that land in different codegen units.
/// A `static` has one address, so the blob ships once however many scaffolds
/// reference it.
pub static AGENTS_BODY: &str = concat!(
    "\n",
    include_str!("architecture.md"),
    r#"
## Access posture — declared per operation, never inferred

Every route, query, message and tool declares its posture, and an operation that
declares none is flagged at boot. There are two declarations, and the answer
picks between them — not the caller:

- `#[authorize(Action, Entity)]` — the class-level gate **plus** automatic
  field-level masking of the value returned. Reach for it whenever the answer is
  entity data, and return the `#[expose]`d wire type (wrapped in `Json<T>` where
  the transport needs it) so the mask has a shape to work on.
- `#[public]` — no gate and no mask. It says *this operation* has no entity to
  gate; it does not say the caller is unauthenticated.

**Rows are ability-scoped, and the guards are what install the scope.** `Repo`
filters every read by the caller's ambient `Ability`; with none installed a read
denies every row rather than returning them unscoped. So a scaffolded adapter is
not "open" — it answers nothing until it is wired, which is the fail-secure half
of the design. Before an operation serves real rows: swap `#[public]` for
`#[authorize]`, bind `#[use_guards(AuthnGuard, AuthzGuard)]` on the struct, and
import the edge's authz module in that adapter's `module.rs` —
`AuthzModule`, `AuthzGraphqlModule`, `AuthzWsModule`, `AuthzMcpModule`.
`nestrs g auth` writes all of them, and `nestrs g <edge>` on a `g resource` port
emits the wiring already done.

Two edges answer differently, by design. A **scheduled** tick is system work:
`SeaOrmDatabaseModule`'s job context installs the executor with no ability, so `Repo`
runs unscoped and there is nothing to declare. **MCP** gates at the endpoint,
through the app's `dyn McpOperationGuard`, else the global guard pool, else
deny-all — so an `/mcp` endpoint with no bridge registered answers 401 to every
tool call.

**`AuthzAbility` is the whole policy, and it grants nothing until you say so.** A
freshly scaffolded resource answers 403 on every route until a rule names its
entity:

```rust
ab.can(Action::Read, post::Entity);
ab.can(Action::Manage, post::Entity)
    .when(|p| p.eq(post::Column::AuthorId, actor.sub));
```

`define` answers for an authenticated actor; `define_visitor` answers for a
caller with no token, on a `#[public]` route only — a `#[public]` route reached
*with* a valid token uses `define`. `nestrs g resource` prints the lines to paste
for the resource it just generated.

## Errors

**`thiserror` in a library, `anyhow` at the binary's entry point.** A service
returning `anyhow::Result` hands its caller a string: the transport can only
stringify it, and no caller can tell "not found" from "the backend is down".
Domain errors are an enum in `error.rs` — never scattered through
`service.rs` — and they propagate as `Result` all the way to the transport
boundary, which maps them to a status code.

## Configuration

Every module's config is settable **both** ways: from the environment and
pinned in code. A field that only exists in one of the two is incomplete.

**Never spell a variable name as a literal** — not in a message, not in a
check, not in a test. `{{env_prefix_var}}` is set on the process and
renames every framework variable at once, so a name typed by hand points at
nothing the day it changes, and the compiler never notices. Build it
(`nest_rs_config::var_name`, `EnvPrefix::var`) or name the setting in words.

## Dependencies

**One framework line.** `cargo add nest-rs --features <capability>` — never a
`nest-rs-*` sub-crate. The manifest names only what your own source names.

## Observability

A constant event-name message plus structured fields, never interpolation —
the output is JSON. `tracing::info!(target: "{{span_target}}", user_id = %id,
"created user")`, not a formatted sentence. **Every event carries at least one
field**; a bare log is a defect, and the events queried under an incident are
exactly the ones people emit bare. Controllers log `info` on success, services
`debug`, denials and security events `warn` or above.

**The target is rooted at the crate that emits, never at the product.** One
target per concern per crate: `{{span_target}}` here. A crate whose name is
not the product's keeps its own root anyway — the target's one job is to say
where the event came from.

## Testing

A test target is always a directory — `tests/<suite>/main.rs`, even for one
file. Exactly two suite names: **`integration`** (in process, no live infra)
and **`e2e`** (needs infrastructure, selected by the nextest binary filter,
never `#[ignore]`). Inside a suite the module tree mirrors `src/`, and
`main.rs` holds the `mod` list and shared fixtures — no test function.
Unit tests stay in `#[cfg(test)] mod tests` in the file under test.

The scaffolded `tests/integration/main.rs` boots the **feature's own** module
through `TestApp`, never the app root: the root grows every transport and
connection the app serves, and the moment it imports `SeaOrmDatabaseModule` a suite
defined as infrastructure-free waits 30 s for a pool. Assert on the composed app
in `tests/e2e/main.rs` instead — scaffolded empty, and where a test boots against
a throwaway database with `nest_rs::testing`'s `EphemeralDatabase` (feature
`orm`) before driving it through `TestApp` the same way.

## Commands

`nestrs run` is the single front door: `dev`, `start`, `build`, `lint`,
`check`, `test <unit|e2e|cov|doc>`. This project's framework variables carry
the `{{env_prefix}}_` prefix.
"#
);

/// The `e2e` suite, scaffolded **literally empty** for every app in either
/// layout — it exists so the two filtersets resolve, and a suite with no tests
/// yet has nothing to say that its own header would not be guessing at.
///
/// `nestrs run test unit` filters on `not binary(e2e)` and `test e2e` on
/// `binary(e2e)` — nextest rejects a filterset naming a binary the workspace
/// does not have, so the suite has to exist from day one for either command to
/// run at all. What belongs in it — the tests needing live Postgres, Redis or
/// object storage, booted against a throwaway database through
/// `nest_rs::testing`'s `EphemeralDatabase` — is stated in the generated
/// `AGENTS.md`.
pub const E2E: &str = "";

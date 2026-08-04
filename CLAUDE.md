# CLAUDE.md — nestrs

Durable decisions. The code says what *is*; this file says what was
**decided** and must be **respected**. Not a code map — layout,
signatures and versions live in the code.

Public repo. No machine-local paths, no private references.

Zone-specific rules load on demand from `.claude/rules/` when you touch
a matching file. This file is the always-loaded core: thesis,
invariants, naming, and what "done" means.

## Thesis

nestrs is an opinionated Rust framework whose thesis is **the developer
writes business logic; the framework carries the rest**. Cross-cutting,
error-prone concerns — **authn, authz, row-level filtering,
transactions, edge validation, discovery, lifecycle** — must be
*transparent*. Forcing the developer to hand-manage any of them is a
framework defect.

The leverage is **procedural macros** — decorators, as declarative in
Rust as in TS. Reach for one first.

## Rule priority — Rust first, conventions second

Both, in order. When they conflict, **Rust wins** — adapt the
convention, don't bend Rust.

1. **Rust (non-negotiable).** Idiomatic, reviewable: orphan/coherence,
   explicit errors (`thiserror` in libs — no silent failure, no
   swallowed `DbErr`), **zero `unwrap`/`expect` on framework hot paths**
   (tests and one-shot bootstraps may use them), honest APIs
   (`Type::new(deps)` when tests need it), `Result` propagated to the
   transport boundary. Macro-emitted `impl` blocks don't excuse hiding
   errors or bypassing `Repo`.
2. **Conventions (second).** Module/feature folders, decorator names,
   thin handlers, one `service.rs` per feature. Conventions = *where*;
   Rust = *how*.

## Hard "no" list

Violating one is never a shortcut — it is a defect. If a task appears
to require it, **stop and ask**.

- **No authn/authz decision outside a guard.** Only `#[use_guards]` + a
  visible `#[authorize]`/`#[public]` declare posture. A parameter type
  (`Authorized<A, E>`), a service method, or a binding helper is never
  the check. Every check must be greppable as one of those three sites.
- **No data access outside a service; no service reaching the DB outside
  `Repo`.** The named exceptions are listed in the data-layer rule;
  there are no others.
- **No silent failure.** Never return `[]`/`None` when the DB errored —
  batch and loader methods return `Result`. Never log-and-pretend-success.
- **No external DI library.** Ours is internal by decision. Extend it.
- **No renaming the umbrella crate.** The facade stays `nest-rs`, every
  sub-crate `nest-rs-*` (paths `nest_rs_*`, span targets
  `nest_rs::<concern>`). The `nestrs` brand (CLI, `NESTRS_*` env,
  nestrs.dev) deliberately differs — accepted, not a bug to fix.
- **No env-var name spelled as a literal.** `NESTRS` is the *app's*
  default prefix, not a fixture: `env_prefix!("ACME")` renames every
  framework variable at once. So a name is always built —
  `nest_rs_config::var_name(ns, key)` or `EnvPrefix::var(name)`, never
  `"NESTRS_AUTHN__SECRET"` in a message, a check or a template. Two
  exceptions, both because they are not the app's: `RUST_LOG` (the
  ecosystem's) and `NESTRS_NO_BOOTSTRAP` (the CLI tool's own).
- **No collapsing the two workspaces.** `demo/apps/` and
  `demo/crates/features/` are fixed names.
- **No feature flags for capabilities that don't exist yet.**
- **No decorator that forces a manifest line.** A macro expansion never
  obliges the developer to declare a crate. See *The umbrella is the
  front door*.
- **No backwards-compatibility shims** — no public API to preserve yet.
- **No mocking the database in e2e tests.**
- **No flat `tests/<x>.rs` and no third suite name.**
- **No umbrella module re-exporting every edge of a feature.**
- **No transport-level discovery without module-gating.**
- **No two decorators for the same concern** — deprecate first.
- **No new third-party crate without a release in ~12 months.** A
  failing candidate must be flagged explicitly, never adopted silently.
- **No third-party version requirement outside `major.minor`.** Two
  components, everywhere, in every manifest the repo owns *and*
  generates: the minor is the floor we build against, the patch is the
  publisher's. `"1"` and `"1.53.1"` are both defects. Move the minor
  with `cargo update`; a semver-*incompatible* release is reported to
  the owner, never taken. One documented exception, at its pin. See
  `.claude/rules/manifests-ci.md`.

## Two workspaces — framework vs. product

- **`crates/nest-rs-*` (root workspace) — the framework.** Generic,
  publishable, product-agnostic. Never names a concrete `Claims`,
  entity or policy — generic *over* them. No runnable app.
- **`demo/` — the product** (the "Publish" demo). Its own workspace
  (`apps/*` + `crates/*`), consuming the framework by **relative path**.
  `cd demo` and drive it as its own repo: `nestrs run`, `.env` cascade,
  `Justfile`, its own `Cargo.lock`/`target/`.

Each builds, tests and locks on its own. A change spanning both compiles
in `demo/` — the path dep pulls live framework source.

**Dividing rule:** `demo/crates/features/` when *any other app could
reuse it*; `demo/apps/<x>/` only when *this app's exposure decides
something the feature can't generalize*.

## The umbrella is the front door

A developer installs **one** crate — `nest-rs`, with the feature for the
capability — and writes code. Cargo resolves the rest. This is the norm
for Rust frameworks (tokio, bevy, rocket, tauri), and it is what makes
the *thesis* true at install time, not just at call time.

- **A macro never makes the developer declare anything.** Every path an
  expansion needs is rooted at `::nest_rs::<concern>::` — never at a
  sibling crate. Seams that are not public API stay `#[doc(hidden)]`
  where they already live; the root is what matters, not the module.
  A decorator whose expansion forces a second `nest-rs-*` line into the
  consumer's manifest is a framework defect, not a documented caveat.
- **Sub-crates are compilation units, not the install surface.** They
  stay published and `nest-rs-*` named; the docs stop presenting them as
  an entry point. Renaming the `nest-rs` dependency is unsupported —
  the same limit tokio has, for the same reason.
- **"The use site owns that crate by definition" is not a reason to make
  it declare one.** Owning a capability means enabling its feature. That
  argument, once accepted, justifies every line in a stanza.
- **The developer's manifest names only what *their own source* names.**
  `serde`, `anyhow`, `sea-orm` stay when their code writes them; every
  `nest-rs-*` beyond the umbrella goes. Shrinking past that is
  over-reach — it costs them feature control and a readable `cargo tree`.
- **A capability that cannot hold "one dependency" is reported**, never
  absorbed into prose in an `## Install` stanza. Adapting that prose is
  what produced the state this rule replaces.

**Shipping a new capability** means all of this, or it is not shipped:

1. A feature in the umbrella's matrix pulling **everything its decorators
   emit unconditionally** — a satellite left out is an `E0433` inside a
   macro expansion, blamed on the attribute.
2. A `pub use nest_rs_<x> as <x>;` re-export, so `::nest_rs::<x>::` resolves.
3. `cargo add nest-rs --features <x>` in **both** the crate README (the
   crates.io landing page) and the docs page's `## Install`. A README
   saying `cargo add nest-rs-<x>` is an unfinished capability.
4. Any derive the decorator emits routed through the surface crate **with
   its `crate = ` override**, so the use site declares neither the crate
   nor a version to keep aligned.
5. Two witnesses, and they are **inside the framework** — there is no
   snippet crate outside the workspaces, and adding one is out of scope:
   - **The expansion** — a use site in `nest-rs-macro-hygiene`, whose one
     dependency proves a decorator needs no second manifest line. It holds
     *decorators only*: a module import there proves nothing about a
     macro, and squats a proof that belongs next door.
   - **The composition** — a test in the capability's **own crate** that
     boots the documented wiring through `nest_rs_testing::TestApp` (or
     `App::builder`) and asserts what a caller gets back. Every `for_root`
     seam has one; that is the bar. Composition is *executed*, never
     merely compiled — a boot test also proves the access graph, the
     resolved config and the mounted routes, which compiling cannot.

   This mirrors the split every framework of this shape settles on:
   `@nestjs/testing` + `sample/` — a testing module and real example
   apps, not a parallel scratch project. `demo/` is our `sample/`; a page
   whose snippet has no counterpart in `demo/` or in the owning crate's
   suite is undocumented, not under-tooled.

   **Assert against shared constants, never a copied literal.** A test
   that re-types `"posts:read audio:transcode"` passes while the policy
   and the deployment drift apart; one that reads
   `features::authz::constants::ALL` fails the moment they do.

**One exception, and only one: binaries.** `cargo add` puts a *library* in
a manifest; `cargo install` puts a *command* on `PATH`. `nestrs` is a
command, so `cargo install --locked nest-rs-cli` is correct and there is no
`--features cli` that could replace it — the same split every Rust tool has.

The compile-time witness is `nest-rs-macro-hygiene`: one dependency,
`nest-rs`, all features. If its manifest needs a second line, the rule
is broken.

## Naming — strict

File name = role; folder = feature prefix (`users/service.rs`).
Snake_case, no dotted variants. **One role → one file per folder.**

| Role | File |
|---|---|
| DI module (exactly one `#[module]` struct per file) | `module.rs` |
| Folder index (`pub use` / `mod` only) | `mod.rs` |
| Service | `service.rs` |
| Controller (REST) / Resolver (GraphQL) / Gateway (WS) | `controller.rs` / `resolver.rs` / `gateway.rs` |
| Processor (queue) / Scheduled tasks / Tool (MCP) | `processor.rs` / `tasks.rs` / `tool.rs` |
| Event listener host | `events/listener.rs` |
| Entity (ORM + `#[expose]`) | `entity.rs` / `entities/` |
| Guard / Strategy | `guard.rs` / `strategy.rs` |
| Domain-specific error / Static constants | `error.rs` / `constants.rs` |

- **`module.rs` is the DI module; `mod.rs` is the folder index.** Never
  merge. **No `*_module.rs` ever.**
- **`mod.rs` / `lib.rs` carry no business logic** — only `//!`, `mod`,
  `pub use`. Exception: proc-macro entries (Rust forces them at the
  crate root) must be thin delegations.
- **A service's type ends in `Service`; one service per `service.rs`.**
  A business-logic provider not ending in `Service` is mis-modeled.
  Being injectable doesn't make a provider a service — a client, config,
  guard, strategy or pipe is a *plain provider* with a role-descriptive
  name.
- **Injected service field = `svc`** when a struct has exactly one;
  `<name>_svc` when several or ambiguous (`users_svc`, `jwt_svc`).
  Non-service deps keep descriptive names (`db`, `queue`, `config`).
- **Same-role plural ⇒ pluralized sub-folder** (`pipes/`,
  `strategies/`); the singular trait file stays at the parent.
- **No `interfaces/` directory** — a trait lives with its concern.
- **Errors in `error.rs`** — not scattered inside `service.rs`.
- **A file exists only if it has real content.**

## Engineering posture

- **No premature abstraction.** Extract after a pattern appears twice.
- **Strict typing.** Enums over string states. Parse at the edge
  (`validator`, `uuid` v7). Newtypes for *meaning*, not format. Avoid
  `Box<dyn Any>` / `serde_json::Value` passthrough.
- **Errors at boundaries**: `thiserror` in libs, `anyhow` at app entry.
- **Doc comments only when the *why* is non-obvious** — never paraphrase
  the name.
- **Security is primordial**: denials and security events log at
  `warn`+, never `debug`.
- **One way to do a thing.** Deprecate before adding a second.

## Observability

- **Span targets dotted, lowercase, framework-prefixed**: `nest_rs::http`,
  `nest_rs::orm`, `nest_rs::authn`, … One target per concern per crate.
  App spans use the app name (`api::users`); the shared feature library
  uses `features::<snake>` (the style the CLI scaffolds).
- **Level per layer.** Controllers/resolvers/gateways: `info` on success.
  Services: `debug`. `Repo`: `trace`. Denials/security: `warn`+.
  Unexpected errors: `error`.
- **Message + fields, never interpolation.** Output is JSON: a constant
  event-name message (`"mounted route"`, not `"GET /v1/users mounted"`)
  plus dynamic data as **structured fields**. Never bake values into the
  message or hand-format columns.
- **Metadata is mandatory — a bare log is a defect.** Every event carries
  ≥1 structured field. A `warn`+ denial emitted bare is a security gap,
  not a style nit: those are the events queried under incident.
- **One event, said once.** Don't restate what a field or the enclosing
  span carries; don't emit the same event at two layers.

## Testing

Wiring bugs don't surface in unit tests.

**The devcontainer provides live backends — e2e infra is ALWAYS
reachable here.** Postgres (`postgres:5432`), Redis (`redis:6379`), S3
(`rustfs:9000`) are `depends_on: service_healthy`, up before you get a
shell; `demo/.env` wires those hostnames. **Never claim they are
unreachable and skip e2e on that basis** — a recurring, *false*
assumption (owner-confirmed 2026-07-09). A real connection failure is a
regression to report, not an environment limit.

**THE test-layout norm — locked 2026-07-09, do not reopen.** A finding
that seems to justify a change goes to the owner as a *question*, never
as an edit.

1. **A test target is always a directory: `tests/<suite>/main.rs`** —
   even for one file. A flat `tests/<x>.rs` is forbidden: Cargo compiles
   it as its own binary, escaping the nextest gates and relinking per
   file.
2. **Exactly two legal suite names.** `integration` — the crate's public
   API in process, no DB/network. `e2e` — needs live infra, gated by the
   nextest filter `binary(e2e)`, **never** `#[ignore]`.
3. **Inside the suite the module tree mirrors `src/`.** `main.rs` is the
   suite *root*, never a test module: `//!` + the `mod` list + the
   fixtures the siblings share (`crate::…`) — **no `#[test]` function
   lives there.** A test belongs to the module named for the `src/`
   concern it covers, so "where is this asserted?" has the same answer as
   "where is this implemented?". One exception: `nest-rs-testing`
   organizes by concern.
4. **Unit tests are untouched** — `#[cfg(test)] mod tests` in the file
   under test.
5. **The runner is nextest.** Bare `cargo test` is unsupported except
   `--doc`.

## Definition of done

Only call a task done when these pass for **every workspace touched**.
Report what ran; never claim a step you skipped. **Show evidence, don't
assert success.**

**Framework (root workspace)** — no `Justfile` here, cargo directly:

```
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check
cargo nextest run --workspace -E 'not binary(e2e)' && cargo test --workspace --doc
cargo nextest run --workspace -E 'binary(e2e)'   # if it touches seaorm/storage
```

**Product (`demo/`)** — `nestrs run` is the single front door:

```
nestrs run lint
nestrs run test unit
nestrs run test e2e      # if it touches transports, DI wiring, or persistence
```

**HTTP/GraphQL changes need one more step**: run the binary, `curl` the
affected endpoints, confirm the response, then **kill the server before
returning control**. Tests passing is not evidence a route is mounted.

GraphQL apps commit their SDL (`apps/<app>/schema.graphql`), regenerated
as a side effect of the dev run — no standalone generator, no CI check.

## Autonomous work — stop and ask

These are owner decisions. In a `/loop` or unattended run, **halt and
surface the question** rather than pick:

- Anything on the *Hard "no" list* that the task appears to require.
- Reopening a locked decision (test layout, workspace split, crate
  naming).
- A new third-party dependency.
- A second way to do something a decorator already does.
- A migration that drops or rewrites existing data.
- A documented rule that has drifted from the code — report it; don't
  edit either side to match.

**Progress rule:** if two consecutive iterations make no measurable
progress against *Definition of done*, stop and report the blocker
instead of trying a third variation.

## Workflow

State the plan in one or two sentences before tools. Batch independent
calls in parallel. Run the *Definition of done* sequence for every
workspace you touched. Report what changed and what was verified — no
paragraph-long summary.

## Reading order

This file plus the **code** are the source of truth.

1. **This file** — durable rules.
2. **`demo/crates/features/src/users/`** — reference feature; copy
   before inventing. If the copy isn't enough, fix the exemplar — don't
   invent a second pattern.
3. **`demo/apps/api/module.rs`** — canonical composition.

User-level IDE rules (e.g. "explain in French, code/comments in
English") apply per session.

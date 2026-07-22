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
- **No collapsing the two workspaces.** `demo/apps/` and
  `demo/crates/features/` are fixed names.
- **No feature flags for capabilities that don't exist yet.**
- **No backwards-compatibility shims** — no public API to preserve yet.
- **No mocking the database in e2e tests.**
- **No flat `tests/<x>.rs` and no third suite name.**
- **No umbrella module re-exporting every edge of a feature.**
- **No transport-level discovery without module-gating.**
- **No two decorators for the same concern** — deprecate first.
- **No new third-party crate without a release in ~12 months.** A
  failing candidate must be flagged explicitly, never adopted silently.

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
3. **Inside the suite the module tree mirrors `src/`.** `main.rs` stays
   thin (`//!` + `mod`). One exception: `nest-rs-testing` organizes by
   concern.
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

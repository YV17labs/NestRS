# CLAUDE.md — nestrs

What this repository decided, how its work is done, and what holds each rule.
Public repo: no machine-local paths or private references. `.claude/rules/`
loads by path (`architecture.md` always); `.claude/decisions/` holds the why,
never loaded — read the entry a rule cites before re-proposing what it refused.
How a rule is written, and each file's budget, is `.claude/rules/rules.md`.

## Thesis

The developer writes business logic; the framework carries the rest. Authn,
authz, row filtering, transactions, edge validation, discovery and lifecycle are
transparent — hand-managing one is a framework defect. Decorators are the lever.
The framework builds what keeps an app correct; operating it (pausing, retuning,
dashboards) is the backend's tooling, an issue until an application needs it.

## Which rule wins

1. Security and data integrity — the first hard "no" list.
2. Rust correctness, macro output included: `thiserror` in libs, `anyhow` at
   app entry, `Result` up to the transport boundary, honest APIs
   (`Type::new(deps)`), enums over string states, newtypes for meaning, parsing
   at the edge, no `serde_json::Value` passthrough, every wait the framework
   owns bounded (`container.md`); `unwrap`/`expect` are the workspace lints'.
3. The naming law, settled before any design question.
4. Conventions say where, Rust says how; one way to do a thing; doc comments
   only for a non-obvious why.
5. Speed and convenience.

## How we work

The session the owner talks to is the **lead**: it writes the production code,
integrates, and never grades its own work. Two subagents judge from a fresh
context, each held to its zone by `.claude/hooks/zones.sh`:

- **`qa`** writes the tests that decide done — a failing reproduction before a
  fix, acceptance tests from a spec — and verifies against them; it never edits
  production code, and nobody weakens a test it wrote.
- **`security`** attacks a change that touches authn/authz, data, persistence,
  concurrency or shutdown, or a value that can reach a reply, a log line or a
  record (`/audit`). It proves, never fixes.

| Task | Route |
|---|---|
| Bug | `qa` reproduces it → the lead fixes the cause and its family → `qa` verifies; `/audit` when it is risky |
| Feature | a one-page spec — goal, non-goals, public names (`/name`), files, tests, the command that says done; the owner approves it only when it changes or breaks the public API, adds a dependency or touches a locked decision → `qa` writes the acceptance tests → the lead builds, with docs and a changelog entry → `qa` verifies; `/audit` when it is risky |
| Dependencies, release | `/deps`, `/release` |
| No behaviour change | the lead, then `just pre-commit` |

**Findings are fixed, not filed.** `/code-review` reads every diff before it
reaches the owner's branch. P0 and P1 are fixed with the test that proves them,
then re-checked once — still standing, it is a design question for the owner;
P2 is fixed when cheap, else reported with its evidence; P3 is dropped unless
trivial. Code and a rule that drift: the code wins and the prose follows in that
commit, unless the rule is a security invariant or a hard "no".

**Local first.** Work on `<type>/<slug>` in a worktree under
`.claude/worktrees/`, from the integration branch; a Conventional Commit subject
in the log's declarative style, no AI attribution trailer; fast-forward into the
owner's branch once `just ci` is green. CI runs the same checks on push as the
safety net. Push, tag, publish and anything posted outside are the owner's.

**Ask the owner only for** a hard "no", a locked decision (test layout,
workspace split, crate naming), a new third-party dependency, a public API
break, a migration dropping or rewriting data, or anything leaving the machine.
Decide every other trade-off on performance, security, solidity for developers
and the standards, with the evidence. Two rounds that do not shrink the open
items: stop, and report the blocker with the proposed decision.

**When it breaks**: a red check is the change's that turned it red, reverted
unless fixed at once; a flaky or weak test is `qa`'s; an advisory is `/deps`'s;
two rules that disagree are settled in the commit that meets them.

## How a rule is held

By the first rung that can, named in the rule: types (private constructors,
`const` assertions, exhaustive `match`); rustc and clippy (`[workspace.lints]`,
the root `clippy.toml`; an exception is `#[expect(lint, reason = "…")]`, never
`allow`); behaviour tests; review. No test proves a rule by reading Rust source
or `.claude/` (`.claude/decisions/conformance-scanner.md`), and a rule a type or
lint holds is a pointer to it, never prose.

## Naming is the pillar

A name and its path say the same thing — from a path you know the type, from a
type the file — and a name is judged as the path a caller types, against its
siblings. The model is `.claude/rules/architecture.md`, a symlink to
`crates/nest-rs-cli/src/templates/architecture.md`, which every scaffold ships
as `AGENTS.md` — edit the real file. A name that leaves its crate or reaches an
operator is designed as a set, with `/name`.

## Hard "no" — security and data

- **No authn/authz decision outside a guard.** Only `#[use_guards]` plus a
  visible `#[authorize]`/`#[public]` declare posture — never a parameter type
  (`Authorized<A, E>`), a service method or a binding helper.
- **No data access outside a service, and no service reaching the database
  outside `Repo`**, but for the escapes `.claude/rules/data-layer.md` names.
- **No swallowed error.** Propagate it, or handle it visibly: a typed variant,
  or a documented default logged at `warn` with its chain — never `[]`, `None`,
  `false` or a default in its place, nor a log while the caller hears success
  (clippy's `let_underscore_must_use` and `map_err_ignore` hold part).
- **No exposure outside `#[expose]`**, and no serializer-shaped third place.
- **No payload value or secret in an error, log line, stored record or reply**:
  a decode failure says where, what kind and what was expected, never the value.
- **No discovery without module-gating**: what an `inventory` entry mounts or
  exposes serves only through an app importing its module
  (`.claude/decisions/forward-principal.md`).
- **No TLS without verification**, in any client the framework opens.
- **No queue promise stronger than at least once**; a handler is idempotent.
- **No access to apalis's structures outside its public API**, but the read-only
  6.x check in `nest-rs-redis/src/legacy_layout.rs` (held by `clippy.toml`).

## Hard "no" — the project

- No external DI library; extend ours.
- No microservice transport split (an app is one binary serving the edges it
  imports, never RPC: `.claude/decisions/modular-monolith-per-workload.md`), no
  `ClassSerializerInterceptor`, no outbound `HttpModule`/`HttpService` (an app
  injects its own `reqwest`), no bundled `Logger` (`tracing` is the contract).
- No renaming the umbrella (`nest-rs`, `nest-rs-*`, `nest_rs::<concern>`); the
  `nestrs` brand (CLI, `NESTRS_*`, nestrs.dev) differs on purpose.
- No env-var name as a literal: `NESTRS_ENV_PREFIX=ACME` on the process renames
  every variable, so a name is built (`nest_rs_config::var_name`,
  `EnvPrefix::var`) and prose writes `<PREFIX>_`. `RUST_LOG`,
  `NESTRS_NO_BOOTSTRAP` and `NESTRS_ENV_PREFIX` (once per crate) are exempt; the
  prefix is set on the process, never in `.env`. `just test` and CI run the
  suites under ACME.
- No collapsing the workspaces; `demo/apps/`, `demo/crates/features/` are fixed.
- No decorator forcing a manifest line, none on two item shapes (a struct host
  plus an impl sibling, `#[controller]`/`#[routes]`, held by
  `nest_rs_codegen::pair::ALL`), and no two for one concern.
- No second way to configure a module: `Module::for_root(x)`, one value, an
  opaque `*Setup` — no builder chain, other constructor or env-only `#[config]`.
- No breaking change inside a major; the API a break replaces goes in that
  major, never kept as an alias. A detector for persisted data is not a shim.
- No feature flag for a capability not built, no umbrella module re-exporting
  every edge of a feature, no mocked database in an e2e test.
- No third-party crate without a release in about 12 months, no version
  requirement but `major.minor` (`.claude/rules/manifests-ci.md`), and no
  `#[tokio::main]` (held by `clippy.toml`).

## Two workspaces, one front door

`crates/nest-rs-*` is the framework: publishable, generic over a `Claims`, an
entity or a policy and never naming one. `demo/` is the product, the "Publish"
demo, its own workspace on the framework by path (`nestrs run`). Code goes in `demo/crates/features/` when another app could reuse it,
in `demo/apps/<x>/` when only this app's exposure decides it. Copy
`demo/crates/features/src/users/` before inventing (fix the exemplar, never add
a pattern) and read `demo/apps/api/src/module.rs`, the canonical composition.
`demo/` Rust carries no comments (`.claude/rules/demo.md`).

A developer installs one crate, `nest-rs` with a capability's feature (a binary:
`cargo install --locked nest-rs-cli`). A macro's paths are rooted at
`::nest_rs::<concern>::`, so a manifest names only what its own source names;
shipping a capability is `.claude/rules/manifests-ci.md`.

## Families — design for the family, build for the caller

An ask arrives at one member of a set (edges, backends, decorator pairs): decide
the name, default, grammar or error sentence every member could take (a key goes
through `nest_rs_codegen::Grammar` or a compile error naming what it accepts,
never ignored), build for the caller, list the other members in the commit body.
Security and data-integrity fixes ship at every member reaching the data, in the
same change. Abstract at the third occurrence.

## Observability

Events go on the target constant of the crate owning the concern
(`nest_rs::<concern>`, `features::<feature>`, `<app>::<concern>`): `info` for an
edge's success, `debug` in services, `trace` in `Repo`, `warn`+ for denials and
security, `error` when unexpected; a constant message and at least one field,
never a value in the message; an `error` field is the whole chain
(`nest_rs_core::error_message`). `trace_id`, `span_id` and `actor_id` come from
context, never as fields; `actor_id` is audit, never authorization.

## Testing

Wiring bugs do not surface in unit tests. Postgres, Redis and S3 run in the
devcontainer: one that does not answer is an environment defect to fix, never a
reason to skip e2e. The layout is locked; a finding against it is the owner's.

1. A test target is a directory, `tests/<suite>/main.rs`, never `tests/<x>.rs`.
2. Two suites: `integration` (in process, no database or network) and `e2e`
   (live infra, `binary(e2e)`, never `#[ignore]`).
3. A suite mirrors `src/`; its `main.rs` holds the `mod`s, shared fixtures and a
   framework `//!`, never a `#[test]` (`nest-rs-testing` organizes by concern).
4. Unit tests are `#[cfg(test)] mod tests` in the file under test, the lead's;
   the suites under `tests/` are `qa`'s. nextest runs them; bare `cargo test`
   only for `--doc`.

## Definition of done

`just pre-commit` before each commit (formatting, clippy, every in-process suite
but the compile-fail snapshots, which a `*-macros` or `nest-rs-codegen` change
adds with `just test`); `just ci`, every check CI runs, before a branch reaches
the owner's. An app's `main.rs` or wiring outside `TestApp` moved: run the
binary, `curl` what changed, stop it. fmt is the `.claude/settings.json` hook;
a file changed another way gets `just fmt`. `cargo mutants` is advisory
(`.claude/rules/testing.md`). Report each command run with its summary line.

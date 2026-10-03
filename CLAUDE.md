# CLAUDE.md — nestrs

What this repository decided, and what holds each rule. Public repo: no
machine-local paths or private references. `.claude/rules/` loads by path
(`architecture.md` always); `.claude/decisions/` holds the why, never loaded —
read the entry a rule cites before re-proposing what it refused. How a rule is
written, and each file's budget, is `.claude/rules/rules.md`.

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

## How a rule is held

By the first rung that can, named in the rule: types (private constructors,
`const` assertions, exhaustive `match`); rustc and clippy (`[workspace.lints]`,
the root `clippy.toml`; an exception is `#[expect(lint, reason = "…")]`, never
`allow`); behaviour tests; review. No test parses Rust source or `.claude/`
(`.claude/decisions/conformance-scanner.md`), and a rule a type or lint holds
is a pointer to it, never prose.

## Naming is the pillar

A name and its path say the same thing — from a path you know the type, from a
type the file — and a name is judged as the path a caller types, against its
siblings. A port keeps the bare name and a driver carries its subject
(`nest_rs::throttler::ThrottlerModule`, `nest_rs::redis::RedisThrottlerModule`).
The stem is the crate subject plus every folder below `src/`
(`redis/queue/module.rs` is `RedisQueueModule`), siblings follow one scheme (a
rename leaving its `*Setup` behind is half a rename), and a `#[config]`'s
namespace is the stem joined by `__` (`<PREFIX>_SEAORM__URL`, never
`<PREFIX>_DATABASE__URL`). A composition root has three module shapes, no
fourth: `<Vendor>Module::for_root`, `<Port>Module::for_root`,
`<Vendor><Port>Module`. The model is `.claude/rules/architecture.md`, a symlink
to `crates/nest-rs-cli/src/templates/architecture.md`, which every scaffold
ships as `AGENTS.md` — edit the real file. A name that leaves its crate or
reaches an operator is designed as a set, with `/name`.

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
  imports; binaries share the features crate and the database, never RPC), no
  `ClassSerializerInterceptor`, no outbound `HttpModule`/`HttpService` (an app
  injects its own `reqwest`), no bundled `Logger` (`tracing` is the contract).
- No renaming the umbrella (`nest-rs`, `nest-rs-*`, `nest_rs::<concern>`); the
  `nestrs` brand (CLI, `NESTRS_*`, nestrs.dev) differs on purpose.
- No env-var name as a literal: `NESTRS_ENV_PREFIX=ACME` on the process renames
  every variable, so a name is built (`nest_rs_config::var_name`,
  `EnvPrefix::var`) and prose writes `<PREFIX>_`. `RUST_LOG`,
  `NESTRS_NO_BOOTSTRAP` and `NESTRS_ENV_PREFIX` (once per crate) are exempt; the
  prefix is set on the process, never in `.env`. CI runs the suites under ACME.
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

## Two workspaces — framework vs. product

`crates/nest-rs-*` is the framework: publishable, generic over a `Claims`, an
entity or a policy and never naming one. `demo/` is the product, the "Publish"
demo: its own workspace on the framework by path — `cd demo` and drive it with
`nestrs run`. Code goes in `demo/crates/features/` when another app could reuse
it, in `demo/apps/<x>/` when only this app's exposure decides it. Copy
`demo/crates/features/src/users/` before inventing (fix the exemplar, never add
a pattern) and read `demo/apps/api/src/module.rs`, the canonical composition.
`demo/` Rust carries no comments: the owner's rule, `.claude/rules/demo.md`.

## The umbrella is the front door

A developer installs one crate, `nest-rs` with a capability's feature (a binary:
`cargo install --locked nest-rs-cli`). A macro never makes them declare anything
— its paths are rooted at `::nest_rs::<concern>::` — and their manifest names
only what their own source names; a capability that cannot hold one dependency
is reported. What shipping one takes is `.claude/rules/manifests-ci.md`.

## Families — design for the family, build for the caller

An ask arrives at one member of a set (edges, backends, decorator pairs). Decide
for the family: a name, default, grammar or error sentence every member could
take; a decorator key goes through `nest_rs_codegen::Grammar` or is refused by a
compile error naming what it accepts, never ignored. Build for the caller: list
the other members in the commit body (fixed, not affected, or an issue); build
or refuse nothing for a member nobody uses, and never let a site that cannot cap
one that can. Security and data-integrity fixes ship at every member reaching
the data, in the same change. Abstract at the third occurrence.

## Observability

Events go on the target constant of the crate owning the concern
(`nest_rs::<concern>`, `features::<feature>`, `<app>::<concern>`): `info` for an
edge's success, `debug` in services, `trace` in `Repo`, `warn`+ for denials and
security, `error` when unexpected; a constant message and at least one field,
never a value in the message; an `error` field is the whole chain
(`nest_rs_core::error_message`). `trace_id`, `span_id` and `actor_id` come from
context, never as fields; `actor_id` is audit, never authorization. The model is
`.claude/rules/observability.md`.

## Testing

Wiring bugs do not surface in unit tests. Postgres (`postgres:5432`), Redis
(`redis:6379`) and S3 (`rustfs:9000`) are up before a devcontainer shell opens:
a connection failure is a regression, never a reason to skip e2e. The layout is
locked; a finding against it is a question for the owner.

1. A test target is a directory, `tests/<suite>/main.rs`, never `tests/<x>.rs`.
2. Two suites: `integration` (in process, no database or network) and `e2e`
   (live infra, `binary(e2e)`, never `#[ignore]`).
3. A suite mirrors `src/`; its `main.rs` holds the `mod`s, shared fixtures and a
   `//!`, never a `#[test]` (`nest-rs-testing` organizes by concern).
4. Unit tests are `#[cfg(test)] mod tests` in the file under test, and the
   runner is nextest — bare `cargo test` only for `--doc`.

## Definition of done

CI, on every push and pull request against real backends, is the gate: done
means green. Locally, before each commit — about a minute:

```
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace -E 'rdeps(<crate>) & !binary(e2e) & !test(/_diagnostics$/)'
```

- fmt is the `.claude/settings.json` hook, on each `.rs` you edit; a file
  changed another way (sed, a generator) gets `cargo fmt --all`.
- A `*-macros` or `nest-rs-codegen` change adds
  `-E 'rdeps(<crate>) & test(/_diagnostics$/)'`; an adapter change adds
  `-E 'package(<crate>) & binary(e2e)'`.
- `demo/`, or an API it uses, moved: `nestrs run lint && nestrs run test unit`.
- An app's `main.rs` or wiring outside `TestApp` moved: run the binary, `curl`
  what changed, kill it before returning.
- `cargo mutants` is advisory (`.claude/rules/testing.md`); a release adds
  `/security-review`. Report each command run with its summary line.

## Reviews

`/name` before a name leaves its crate; `/architecture` when a territory opens;
`/audit` once, on a change touching authn/authz, data access, persistence,
concurrency or shutdown, or a published API; `/simplify` on the diff, last. A
finding gets its regression test; a rule changes only on a contradiction or a
recurrence. When code and a rule drift, the code wins and the prose follows in
that commit — unless the rule is a security invariant or a hard "no".

## Autonomous work

Decide trade-offs yourself — performance, security, solidity for developers,
the standards — and report the evidence; an unbuilt option is an issue. Stop and
ask only for a hard "no", a locked decision (test layout, workspace split, crate
naming), a new third-party dependency, a second way to do what a decorator does,
a migration dropping or rewriting data, or anything leaving the machine (push,
merge to `main`, tag, publish, upstream post). After two rounds that do not
shrink the open items, stop and report the blocker with your proposed decision.

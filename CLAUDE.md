# CLAUDE.md — nestrs

NestRS is a NestJS-style framework for Rust: modules, dependency injection and
decorators (proc macros) over poem, sea-orm, async-graphql, rmcp and apalis.
Public repo: no machine-local paths or private references.

This file is the map. The rules for a part of the tree live in
`.claude/rules/` and load with the files they cover (`architecture.md` always);
`.claude/decisions/` holds why a design was chosen and what was refused — read
the entry a rule cites before re-proposing what it refused.

## Identity — read first, decide with it

nestrs is a backend framework by developers, for developers: the developer
writes only business logic, and everything else — authn, authz, row filtering,
transactions, validation at the edge, discovery, lifecycle, observability — is
already thought through and runs behind a decorator. Where others write
conditions, a nestrs developer declares `#[authorize]` and moves on; a
cross-cutting concern the developer must hand-manage is a framework defect.
Few lines, nothing to wire, elegant to read: that is how we beat the
competition.

We chose Rust for what it gives, and these values order every trade-off:

1. **Security** — safe by default; an opening is an explicit, visible line.
   Security standards (RFCs, OWASP) are followed, never improvised. The hard
   "no" lists below are this value made concrete.
2. **Correctness** — Rust's guarantees kept, macro output included:
   `thiserror` in libs, `anyhow` at app entry, `Result` up to the transport
   boundary, honest APIs (`Type::new(deps)`), enums over string states,
   newtypes for meaning, parsing at the edge, no `serde_json::Value`
   passthrough, every wait the framework owns bounded (`container.md`).
3. **Performance** — raw speed, close to the machine, measured against Node,
   PHP and Go backends: work moves to compile time (macros) rather than per
   request, and the hot path pays for nothing it does not use.
4. **Elegance** — what a human understands at a glance. Standard names
   (`naming.md`, `architecture.md`); one responsibility per module, service and
   file, so each thing has exactly one place to be found; one way to do a thing.
   Standards over invention: an existing specification, even a recent one,
   beats a homemade scheme.
5. **Our own speed and convenience** — last, never at the expense of the above.

We stay lean, a startup outpacing heavy incumbents: we support the most-used
backends and drivers in their latest versions, not every one, and drop the old
fast — through semver: removing support is a major, and the previous major
keeps it. We build what keeps an app correct; operating it (dashboards,
pausing, retuning) is its backend's tooling. No feature for a hypothetical
user. We aim for excellence, not for done.

**Arbitrate with this before asking.** Most questions answer themselves here:
decide, and say which value decided. Ask the owner only for what *How we work*
lists.

## Layout and commands

- `crates/nest-rs-*` — the framework, one workspace. `nest-rs` is the umbrella
  users install; `*-macros` are proc-macro crates, `nest-rs-codegen` their
  shared code; `nest-rs-cli` is the `nestrs` binary.
- `demo/` — the "Publish" product, its own workspace on the framework by path,
  driven with `nestrs run`. Nothing at the root builds or tests it.
- `bench/` — a standalone benchmark against NestJS. `docs/` — nestrs.dev.
  `changelog/` — one file per minor line, indexed by `CHANGELOG.md`.

```bash
just lint   # fmt, clippy, each capability alone, dependency policy
just test   # every test (nextest) + doctests, against Postgres, Redis and S3
just doc    # rustdoc, warnings denied
just ci     # every check CI runs: lint, docs and tests
```

## How we work

- A bug starts with a failing test that reproduces it; the fix covers its
  cause and its family. A feature that changes the public API, adds a
  dependency or touches a decision here is agreed with the owner first.
- Branch `<type>/<slug>`, Conventional Commit subjects stating what is now
  true. `just ci` green before a branch reaches the owner's. Push, tag,
  publish and anything posted outside are the owner's.
- **Ask the owner only for** a hard "no", a decision recorded here, a new
  third-party dependency, a public API break, a migration dropping or rewriting
  data, or anything leaving the machine. Decide other trade-offs on
  performance, security and the standards, with the evidence.
- A red check belongs to the change that turned it red: fixed at once or
  reverted. Code and a rule that drift: the code wins and the rule is fixed in
  that commit, unless the rule is a security invariant or a hard "no".

## How a rule is held

By the first rung that can: types (private constructors, `const` assertions,
exhaustive `match`); rustc and clippy (`[workspace.lints]`, the root
`clippy.toml`; an exception is `#[expect(lint, reason = "…")]`, never
`allow`); behaviour tests; review. No test proves a rule by reading Rust
source, manifests or `.claude/`.

## Naming is the pillar

A name and its path say the same thing, and a name is judged in its set.
`.claude/rules/naming.md` holds the principles for every name;
`.claude/rules/architecture.md` applies them to modules, files and types.

## Hard "no" — security and data

- **No authn/authz decision outside a guard.** Only `#[use_guards]` plus a
  visible `#[authorize]`/`#[public]` declare posture — never a parameter type
  (`Authorized<A, E>`), a service method or a binding helper.
- **No data access outside a service, and no service reaching the database
  outside `Repo`**, but for the escapes `data-layer.md` names.
- **No swallowed error.** Propagate it, or handle it visibly: a typed variant,
  or a documented default logged at `warn` with its chain — never `[]`, `None`,
  `false` or a default in its place, nor a log while the caller hears success.
- **No exposure outside `#[expose]`**, and no serializer-shaped third place.
- **No payload value or secret in an error, log line, stored record or reply**:
  a decode failure says where, what kind and what was expected, never the value.
- **No discovery without module-gating**: what an `inventory` entry mounts or
  exposes serves only through an app importing its module
  (`decisions/forward-principal.md`).
- **No TLS without verification**, in any client the framework opens.
- **No queue promise stronger than at least once**; a handler is idempotent.
- **No access to apalis's structures outside its public API**, but the read-only
  6.x check in `nest-rs-redis/src/legacy_layout.rs`.

## Hard "no" — the project

- No external DI library; extend ours.
- No microservice transport split (an app is one binary serving the edges it
  imports, never RPC: `decisions/modular-monolith-per-workload.md`), no
  `ClassSerializerInterceptor`, no outbound `HttpModule`/`HttpService` (an app
  injects its own `reqwest`), no bundled `Logger` (`tracing` is the contract).
- No renaming the umbrella (`nest-rs`, `nest-rs-*`, `nest_rs::<concern>`); the
  `nestrs` brand (CLI, `NESTRS_*`, nestrs.dev) differs on purpose.
- No env-var name as a literal: `NESTRS_ENV_PREFIX=ACME` on the process renames
  every variable, so a name is built (`nest_rs_config::var_name`,
  `EnvPrefix::var`) and prose writes `<PREFIX>_`. `RUST_LOG`,
  `NESTRS_NO_BOOTSTRAP` and `NESTRS_ENV_PREFIX` are exempt; the prefix is set
  on the process, never in `.env`.
- No collapsing the workspaces; `demo/apps/`, `demo/crates/features/` are fixed.
- No decorator forcing a manifest line, none on two item shapes (a struct host
  plus an impl sibling, `#[controller]`/`#[routes]`, held by
  `nest_rs_codegen::pair::ALL`), and no two for one concern.
- No second way to configure a module: `Module::for_root(x)`, one value, an
  opaque `*Setup` — no builder chain, other constructor or env-only `#[config]`.
- No breaking change inside a major; the API a break replaces goes in that
  major, never kept as an alias. A detector for persisted data is not a shim.
- No feature flag for a capability not built, no umbrella module re-exporting
  every edge of a feature, no mocked database in a test.
- No third-party crate without a release in about 12 months, no version
  requirement but `major.minor` (`manifests-ci.md`), and no `#[tokio::main]`
  (held by `clippy.toml`).

## Two workspaces, one front door

`crates/nest-rs-*` is generic over a `Claims`, an entity or a policy and never
names one. In `demo/`, code goes in `demo/crates/features/` when another app
could reuse it, in `demo/apps/<x>/` when only this app's exposure decides it.
Copy `demo/crates/features/src/users/` before inventing (fix the exemplar,
never add a pattern) and read `demo/apps/api/src/module.rs`, the canonical
composition. A developer installs one crate, `nest-rs` with a capability's
feature; a macro's paths are rooted at `::nest_rs::<concern>::`.

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

## Comments

A comment is written only when you can name what it saves a future reader:
the trap the code cannot show (a dependency's constraint, a security or
concurrency invariant) or the reference that explains code which would
otherwise look wrong (an upstream issue, a `.claude/decisions/` entry). Can't
name it: don't write it. Then one or two lines. History and reasoning go in the
commit message, never beside the code; never paraphrase the code. A public item
gets a one-sentence `///` (the crates deny `missing_docs`), plus an example when
it helps. `demo/` Rust carries none.

## Testing

Wiring bugs do not surface in unit tests. Postgres, Redis and S3 run in the
devcontainer: one that does not answer is an environment defect to fix, never a
reason to skip a test. **Prove each behaviour at the cheapest level that can
prove it** — unit, then in process, then against a live service — and treat a
slow test as a defect: make it cheaper, or cut what a cheaper test already
proves (`testing.md`).

1. A framework crate has at most two suites, never a flat `tests/<x>.rs`:
   `tests/integration/main.rs` runs in process, `tests/e2e/main.rs` against the
   live services, so `just test integration` needs none. Neither is
   `#[ignore]`d. What both use sits in `tests/harness/`, included by path. The
   one exception: a fixture that puts a deliberately invalid declaration in the
   link-time registry (`inventory`) gets its own `tests/<fixture>/main.rs`,
   since every app booted beside it would read it.
2. Each suite mirrors `src/`; its `main.rs` holds the `mod`s, shared fixtures
   and a `//!`, never a `#[test]`.
3. Unit tests are `#[cfg(test)] mod tests` in the file under test. nextest runs
   everything; bare `cargo test` only for `--doc`.
4. `demo/` apps keep the same two suites, driven by `nestrs run test`.

## Definition of done

`just ci` green: formatting, clippy, rustdoc and every test. An app's `main.rs`
or wiring outside `TestApp` moved: run the binary, `curl` what changed, stop it.
Report each command run with its summary line.

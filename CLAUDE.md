# CLAUDE.md — nestrs

Durable decisions. The code says what *is*; this file says what was
**decided** and must be **respected**. Not a code map — layout, signatures,
numbers and versions live in the code and its rustdoc.

Public repo. No machine-local paths, no private references.

Zone rules load on demand from `.claude/rules/` when you touch a matching
file; `.claude/rules/architecture.md` (the naming model) loads always. Why a
rule is what it is — the alternatives tried and refused — lives in
`.claude/decisions/`, which is never loaded: open it when you are about to
re-propose something.

## Thesis

nestrs is an opinionated Rust framework whose thesis is **the developer
writes business logic; the framework carries the rest**. Cross-cutting,
error-prone concerns — **authn, authz, row-level filtering, transactions,
edge validation, discovery, lifecycle** — must be *transparent*. Forcing the
developer to hand-manage any of them is a framework defect.

The framework carries what a developer writes and what makes it correct in
production. Operating a running system — pausing, reviving, retuning,
dashboards — is the backend's own tooling: an issue, never built, until an
application needs it.

The leverage is **procedural macros** — decorators, as declarative in Rust as
in TS. Reach for one first.

## Which rule wins

When two rules pull apart, the higher one wins and nothing else is recorded:

1. **Security and data integrity** — the first block of the hard "no" list.
2. **Rust correctness** — explicit errors (`thiserror` in libs, `anyhow` at app
   entry), no `unwrap`/`expect` on framework hot paths (tests and one-shot
   bootstraps may), honest APIs (`Type::new(deps)` when tests need it),
   `Result` propagated to the transport boundary. Macro-emitted code is held
   to the same bar.
3. **The naming law** — below. It outranks every design and convention
   question, and settles first.
4. **Conventions** — folders, decorator names, thin handlers, one `service.rs`
   per feature. Conventions say *where*; Rust says *how*.
5. **Speed and convenience.**

## How a rule is held

A rule is held by the **first rung that can hold it**, and the rule names its
rung:

1. **Types.** Make the wrong code unwritable: private fields and constructors,
   `const` assertions, exhaustive `match`, a required trait method, a macro
   that only accepts a typed constant.
2. **rustc and clippy.** Workspace lints and the root `clippy.toml`
   (`disallowed-methods` / `-macros` / `-types`). They match *resolved*
   paths, so an alias, a glob, `r#`, `include!` or a macro expansion does not
   evade them. An exception is `#[expect(lint, reason = "…")]` at the site —
   never `allow` — so a stale one fails the build. Every `clippy.toml` entry
   has a canary in `nest-rs-macro-hygiene`, because an entry whose path stops
   resolving is only a warning.
3. **Behaviour tests**, whose assertions `cargo mutants` checks: a test is
   evidence for what it asserts, and a surviving mutant shows what it does
   not.
4. **Review**, against a written sentence.

**No home-made test parses Rust source to prove a rule**, and nothing reads
`CLAUDE.md` or `.claude/`. A source scanner cannot see what the compiler
resolves, so it either misses or grows into a second compiler — 7.0 built one
and removed it (`.claude/decisions/conformance-scanner.md`). What stays in
`nest-rs-conformance` is **structural**: facts read off file paths and
manifests (the naming law, the test layout), where there is nothing to evade.
The CLI reading the `architecture.md` template it ships is product data, not
an exception.

## Naming is the pillar

**A name and its path say the same thing.** From a path you know the type;
from a type you know where the file is. A reader who cannot navigate cannot
check anything else, so this is the most important rule in the project. A name
is **never judged alone** — always as the qualified path a caller types, and
against its siblings at the same level: `ThrottlerModule` is a good name, and
a defect inside `nest-rs-redis`.

1. **A driver carries its subject.** The bare name of a capability belongs to
   the crate that defines the port; a crate implementing somebody else's port
   prefixes its modules with its own subject — `nest_rs::throttler::ThrottlerModule`
   is the port's, `nest_rs::redis::RedisThrottlerModule` is Redis's. The
   stutter is paid on purpose: unambiguous in a log beats short in an import.
2. **The stem is the path.** A module's type name is the crate subject plus
   every folder below `src/`, joined — `redis/queue/module.rs` is
   `RedisQueueModule`, `audio/http/module.rs` is `AudioHttpModule`,
   `posts/http/controller.rs` is `PostsController`.
3. **Siblings follow one scheme.** One odd member means it or the scheme is
   wrong, and deciding which is the finding. A type renamed without its
   `*Setup`, `*Host` or config is half a rename.
4. **A variable is a path too.** A `#[config]`'s namespace is its stem joined
   by `__`: `SeaOrmConfig` reads `NESTRS_SEAORM__URL`, `RedisWorkerConfig`
   reads `NESTRS_REDIS__WORKER__*`. `NESTRS_DATABASE__URL` names neither the
   crate nor the type, and is the defect.

The composition root has **three module shapes, no fourth** —
`<Vendor>Module::for_root` opens a resource, `<Port>Module::for_root` carries a
capability's policy, `<Vendor><Port>Module` binds one to the other — and the
framework is Ports & Adapters: the port owns the semantics, the adapter only
the transport. The full model, the role tables and the reserved vocabulary are
`.claude/rules/architecture.md` — **one file**, the real one in
`crates/nest-rs-cli/src/templates/` (embedded into every scaffold's
`AGENTS.md`) and a symlink in `.claude/rules/`; edit the real one.

Load-bearing enough to repeat: the project name stops at the workspace;
`module.rs` is the DI module and `mod.rs` the folder index and export contract,
never merged, and **no `*_module.rs`**; a file exists only if it has real
content; every error type lives in `error.rs`.

**A name that leaves its crate or reaches an operator is designed as a set** —
a public type, module or crate; an env var or config key; a span target or
unit; a datastore key; a CLI command or flag; a public error variant. Name two
siblings that do not exist yet, sort the list, and say where the next one
goes; `/name` is that procedure. Every other name follows rustc's naming lints
and the path law.

## Hard "no" — security and data

A violation is a defect, never a shortcut. If a task appears to require one,
**stop and ask**.

- **No authn/authz decision outside a guard.** Only `#[use_guards]` plus a
  visible `#[authorize]`/`#[public]` declare posture. A parameter type
  (`Authorized<A, E>`), a service method or a binding helper is never the
  check.
- **No data access outside a service; no service reaching the database outside
  `Repo`.** The named exceptions are in `.claude/rules/data-layer.md`; there
  are no others.
- **No swallowed error.** A fallible call's error is propagated, or handled by
  a decision visible in the code — a typed variant, or a documented default
  logged at `warn` with the error chain. Mapping an error to `[]`, `None`,
  `false` or a default, and logging it while the caller is told it succeeded,
  are the defect. This is what "silent failure" means, and all it means.
  Held by `clippy::let_underscore_must_use` and `clippy::map_err_ignore`, and
  by review.
- **No exposure outside `#[expose]`.** A column is shown or hidden by the
  entity's declaration and the caller's ability, never by a serializer-shaped
  third place.
- **No payload value or secret in an error, a log line, a stored record or a
  reply.** A decode failure is described — where, what kind of value, what the
  type expected — never quoted; a client's value, a producer's job and a
  deployment's config are all somebody's data, and a `400` is kept by proxies.
- **No transport-level discovery without module-gating.** Anything an
  `inventory` submission would mount or expose — a route, a resolver, a tool, a
  loader — serves only through an app that imported its module. A
  request-scoped forwarder mounts nothing; its gate is the guard that produces
  the value (`.claude/decisions/forward-principal.md`).
- **No TLS without verification.** Certificate and hostname verification are
  never configurable off, in any client the framework opens.
- **No queue promise stronger than at least once.** A job may run more than
  once, so a handler is idempotent and no rustdoc, line or page says
  otherwise. Machinery stretching delivery toward exactly-once is the defect.
- **No read or write of apalis's structures outside apalis's public API**,
  except the 6.x boot check in `nest-rs-redis/src/legacy_layout.rs`, which
  reads and never writes. Held by `clippy.toml` on apalis's `Config` getters
  and by an e2e under a read-only Redis ACL.

## Hard "no" — the project

- **No external DI library.** Ours is internal by decision. Extend it.
- **Four NestJS surfaces are refused by design**, each a defect if it
  reappears: a **microservice transport split** (an app is one binary serving
  the edges it imports; two binaries share `demo/crates/features` and the
  database and never RPC each other); **`ClassSerializerInterceptor`**;
  an **outbound `HttpModule`/`HttpService`** (an app injects its own
  `reqwest`; a crate needing a client for its own protocol holds it
  privately); a **bundled `Logger`** (`tracing` is the contract).
- **No renaming the umbrella crate.** The facade is `nest-rs`, sub-crates
  `nest-rs-*` (paths `nest_rs_*`, targets `nest_rs::<concern>`). The `nestrs`
  brand (CLI, `NESTRS_*`, nestrs.dev) deliberately differs.
- **No env-var name spelled as a literal.** `NESTRS` is the deployment's
  default prefix: `NESTRS_ENV_PREFIX=ACME` on the process renames every
  framework variable. A name is built — `nest_rs_config::var_name(ns, key)` or
  `EnvPrefix::var(name)`. Three exceptions, none the app's: `RUST_LOG`,
  `NESTRS_NO_BOOTSTRAP` (the CLI's), and `NESTRS_ENV_PREFIX` itself, spelled
  once per crate that needs it. The prefix is set on the process, never in
  `.env`. Held by the tier-3 run under `NESTRS_ENV_PREFIX=ACME`; prose writes
  `<PREFIX>_`.
- **No collapsing the two workspaces.** `demo/apps/` and
  `demo/crates/features/` are fixed names.
- **No decorator that forces a manifest line** (*The umbrella is the front
  door*).
- **No decorator on two item shapes.** An edge is two decorators — the host on
  the struct, a sibling named for what it collects on the impl
  (`#[controller]`/`#[routes]`, `#[gateway]`/`#[messages]`,
  `#[resolver]`/`#[operations]`, `#[mcp]`/`#[tools]`) — and the wrong shape is
  a compile error naming the sibling. Held by `nest_rs_codegen::pair::ALL`
  and a trybuild snapshot per wrong shape.
- **No two decorators for one concern, and no second way to configure a
  module.** `Module::for_root(x)` takes one value; the `*Setup` it returns is
  opaque; no builder chain, no second constructor, no `#[config]` reachable
  only through the environment.
- **No breaking change inside a major.** A break ships in a major with its
  upgrading entry; a replaced API is removed in that same major, never kept as
  an alias. A detector for persisted data (a stored layout, a wire version) is
  not a shim — data outlives binaries.
- **No feature flags for capabilities that don't exist yet**, and **no
  umbrella module re-exporting every edge of a feature.**
- **No mocking the database in e2e tests.**
- **No third-party crate without a release in ~12 months**, and **no version
  requirement outside `major.minor`**, in every manifest the repo owns or
  generates. Move the minor with `cargo update`; a semver-incompatible release
  is reported, never taken (`.claude/rules/manifests-ci.md`).
- **No `#[tokio::main]`.** Every binary is `#[nest_rs::main]`, whose runtime
  teardown is bounded. Held by `clippy.toml`.

## Two workspaces — framework vs. product

- **`crates/nest-rs-*` (root workspace) — the framework.** Generic,
  publishable, product-agnostic: generic *over* a `Claims`, an entity or a
  policy, never naming one. No runnable app.
- **`demo/` — the product** (the "Publish" demo), its own workspace (`apps/*` +
  `crates/*`) consuming the framework by relative path. `cd demo` and drive it
  as its own repo: `nestrs run`, `.env` cascade, `Justfile`, its own
  `Cargo.lock` and `target/`. A change spanning both compiles in `demo/`.

**Dividing rule:** `demo/crates/features/` when any other app could reuse it;
`demo/apps/<x>/` only when this app's exposure decides something the feature
cannot generalize.

**`demo/` Rust carries no comments** — no `//`, `//!` or `///`, in `apps/`,
`crates/`, tests and `build.rs`. It is demonstration code: if a line needs a
comment, rename it, split it, or move the decision here. A non-Rust demo file
(chart, `Dockerfile`, `Justfile`, manifest, `.env`) keeps its *why* next to the
value, in one short line. A CLI template is the developer's own repository and
may teach in a comment — the `// SECURITY:` note above a generated `#[public]`
route is the case that decides it.

**Prose the framework compiles into behaviour is an argument, not a doc
comment**, in `demo/` and every template: `#[tool(description = "…")]`,
`#[api(summary = …, description = …)]`. The framework itself lets `#[tool]` /
`#[prompt]` fall back to the doc comment; the attribute wins, and an operation
with neither — or a blank one — is a compile error.

## The umbrella is the front door

A developer installs **one** crate — `nest-rs` with the feature for the
capability — and writes code; Cargo resolves the rest, as with tokio, bevy or
tauri.

- **A macro never makes the developer declare anything.** Every path an
  expansion needs is rooted at `::nest_rs::<concern>::`; a `*-macros` crate
  reaches its own surface crate's re-export, never a sibling's. A decorator
  forcing a second `nest-rs-*` line into a manifest is a framework defect.
- **The developer's manifest names only what their own source names**
  (`serde`, `anyhow`, `sea-orm` when their code writes them). Sub-crates are
  compilation units, not the install surface; renaming the `nest-rs`
  dependency is unsupported, as with tokio.
- **A capability that cannot hold "one dependency" is reported**, never
  absorbed into an `## Install` stanza.

**Shipping a capability** means all of this:

1. An umbrella feature pulling everything its decorators emit, and
   `pub use nest_rs_<x> as <x>;`.
2. `cargo add nest-rs --features <x>` in the crate README and the docs page's
   `## Install` (held by the docs lint).
3. Any derive the decorator emits routed through the surface crate with its
   `crate = ` override.
4. **The expansion witness** — a use site in `nest-rs-macro-hygiene`, whose
   one dependency is `nest-rs`, gated on the capability's feature so a
   misgated re-export fails under that feature alone. Decorators only.
5. **The composition witness** — a test in the capability's own crate booting
   the documented wiring through `nest_rs_testing::TestApp` (or
   `App::builder`) and asserting what a caller gets back. Every `for_root`
   seam has one. A boot test through the real transport is the evidence that
   a route is mounted.

`demo/` is our `sample/`: a docs snippet with no counterpart in `demo/` or the
owning crate's suite is undocumented. Tests assert against shared constants,
never a copied literal. The one exception to "one crate" is a binary:
`cargo install --locked nest-rs-cli`.

## Families — design for the family, build for the caller

The framework holds several interchangeable members of many sets — edges,
backends, decorator pairs. An ask arrives at one of them.

- **Decide for the family.** A name, a default, a grammar or an error sentence
  is chosen so every member could take it. A key the framework interprets is
  parsed by `nest_rs_codegen::Grammar` wherever the standard permits it;
  where it does not, it is a compile error naming the decorator and what it
  accepts — one shared sentence, a specific reason when it fits on a line.
  Never an ignored argument.
- **Build for the caller.** Grep the other *existing* members for the same gap
  and list them in the commit body: fixed here, not affected, or an issue. Do
  not build members nobody uses, and do not write refusals or questions for
  hypothetical ones. A site whose standard cannot have the thing never caps
  the sites that can.
- **Security and data-integrity fixes are the exception**: they ship at every
  member that reaches the same data, in the same change.
- **Abstract at the third occurrence**, and prefer a list the compiler holds
  (an exhaustive enum or `match`) to one derived by reading source.

## Engineering posture

- **Strict typing.** Enums over string states; parse at the edge
  (`validator`, `uuid` v7); newtypes for meaning, not format; no
  `Box<dyn Any>` / `serde_json::Value` passthrough.
- **Every wait the framework owns is bounded.** A port call the framework
  awaits has a bound of the framework's, and the way down abandons what is
  still running at its bound, with a line naming it. A gap is listed in its
  crate's `//!` with an issue. The mechanics are in `framework.md`.
- **Doc comments only when the *why* is non-obvious** — never paraphrase the
  name.
- **Security events log at `warn`+**, never `debug`.
- **One way to do a thing.**

## Observability

The full model — W3C trace context, trust, propagation, the OpenTelemetry
deviations — is `.claude/rules/observability.md`. What every crate obeys:

- **Span targets are dotted, lowercase, rooted at the emitting crate**, and
  declared as a constant by the crate that owns the concern:

  | Emitting crate | Target |
  |---|---|
  | a `nest-rs-*` framework crate | `nest_rs::<concern>` — `nest_rs::http`; a family member roots at its family: `nest_rs::oauth::client` |
  | the shared feature library | `features::<feature>` — each feature declares `pub const TARGET` at its root |
  | an app crate, or a single-crate project | `<app>::<concern>` |

  **No target is a raw-string prefix of an unrelated one** — `EnvFilter`
  matches with `starts_with` — held by a structural test over the declared
  constants.
- **Level per layer.** Controllers/resolvers/gateways `info` on success;
  services `debug`; `Repo` `trace`; denials and security `warn`+; unexpected
  errors `error`.
- **Message + fields.** A constant event-name message plus structured fields;
  never a value baked into the message. Every event carries at least one
  field — a bare `warn` denial is a security gap.
- **One event, said once**, and **an `error` field is the whole chain**,
  rendered through `nest_rs_core::error_message`.
- **The formatter escapes every value**, so no field can forge a line.
- **Correlation is W3C Trace Context, in the kernel, never optional.** Every
  log line carries `trace_id`, `span_id` and `actor_id` of its unit of work
  and no span state. `actor_id` is an audit identity, never an authorization
  input.
- **Every edge opens one operation span and files one operation line per unit
  of work**, through `operation_span!` and `operation_line!` over a typed
  `Unit` the edge's crate declares with `unit!` — so a unit name is
  `<edge>.<unit>`, cannot be a literal and cannot drift between the span and
  the line. A unit that does not settle still files its line, as
  `outcome = cancelled` or `panic`. Every operation line is
  `nest_rs::operation`, the family's one toggle.

## Testing

Wiring bugs don't surface in unit tests.

**The devcontainer provides live backends — e2e infra is always reachable
here.** Postgres (`postgres:5432`), Redis (`redis:6379`), S3 (`rustfs:9000`)
are up before you get a shell. Never skip e2e claiming otherwise; a real
connection failure is a regression to report.

**The test layout — locked; a finding against it goes to the owner as a
question.**

1. **A test target is always a directory: `tests/<suite>/main.rs`**, even for
   one file — a flat `tests/<x>.rs` escapes the nextest gates.
2. **Exactly two suite names.** `integration` — the public API in process, no
   database or network. `e2e` — live infra, gated by `binary(e2e)`, never
   `#[ignore]`.
3. **The suite's module tree mirrors `src/`.** `main.rs` is the root: the
   `mod` list, the shared fixtures, and in the framework a `//!` — no
   `#[test]` there. One exception: `nest-rs-testing` organizes by concern.
4. **Unit tests** are `#[cfg(test)] mod tests` in the file under test.
5. **The runner is nextest.** Bare `cargo test` only for `--doc`.

A test is evidence for what it asserts; `cargo mutants` shows what it does
not.

## Definition of done

Report what ran and its summary line; never claim a step you skipped.

**Tier 1 — while editing.** Whatever is fastest for the crates you touch:
`cargo check`, `cargo nextest run -p <crate>`.

**Tier 2 — before each commit** (about a minute):

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings      # workspace-wide is faster than -p
cargo nextest run --workspace -E 'rdeps(<touched crate>) & !binary(e2e)'
cargo nextest run --workspace -E 'rdeps(<touched crate>) & binary(e2e)'    # when it has an e2e suite
git diff HEAD > /tmp/c.diff && cargo mutants --in-diff /tmp/c.diff --test-tool nextest -p <touched crate>   # framework logic
nestrs run lint && nestrs run test unit                     # in demo/, if demo or an API it uses moved
```

A surviving mutant is a behaviour no test asserts: add the test, or say in
the commit body why the mutant is equivalent.

**Tier 3 — before a merge to `main` or a release:**

```
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check
cargo nextest run --workspace && cargo test --workspace --doc
NESTRS_ENV_PREFIX=ACME cargo nextest run --workspace -E '!binary(e2e)'
scripts/check-features.sh                                   # every crate under each of its features alone
cargo audit --file Cargo.lock && cargo audit --file demo/Cargo.lock && cargo audit --file bench/sut/nestrs/Cargo.lock
(cd docs && npm run lint:docs && npm test)
nestrs run lint && nestrs run test unit && nestrs run test e2e   # in demo/
```

Run `cargo audit` from the repository root, where `.cargo/audit.toml` lives.
When an app's `main.rs` or wiring outside `TestApp` changed, also run the
binary, `curl` the affected endpoints, and kill it before returning. A
release adds `/security-review` and the Redis/Valkey version matrix. GraphQL
apps commit their SDL (`apps/<app>/schema.graphql`), regenerated by the dev
run.

## Reviews

Four skills, each with a trigger and a bound.

- **`/name`** — before a name that leaves its crate or reaches an operator
  exists.
- **`/architecture`** — when a territory opens (a new crate, decorator family,
  vocabulary or `for_root` seam), on a scope, never a diff.
- **`/audit`** — **once** on a change that touches authn/authz, data access,
  persistence or transactions, concurrency or shutdown, or a published API.
  Severity is impact: **P0** security or data loss, **P1** a wrong answer
  under the contract, **P2** availability or performance, **P3** ergonomics;
  silence raises a finding within its level, never across. The contract is one
  process crash, one backend timeout or restart, a misconfiguration, a
  hostile client or payload — two independent failures at once is a limit
  written in the crate's `//!`, not a defect. P0/P1 are fixed with a
  regression test, P2 becomes an issue, P3 is dropped unless trivial. Only the
  lines a P0/P1 fix touched are audited again, once; a P0/P1 still standing
  after that is a design problem — stop and bring the redesign.
- **`/simplify`** — on the diff, last.

Run them in that order: a move invalidates every probe taken before it, and a
probe invalidates the polish.

**A finding is fixed with its regression test.** A rule is added or changed
only when two rules disagree or the same class recurs in an unrelated change,
and it names its rung (*How a rule is held*). **When the code and a rule
drift**, the code wins and the prose is corrected in the same commit — unless
the prose states a security invariant or a hard "no", in which case the code is
fixed.

## Autonomous work

Decide engineering trade-offs yourself, by four criteria held together:
performance, security, solidity for the developers building on the framework,
and the relevant standards. Report the decision with its evidence; an unbuilt
option is an issue, not a paragraph in the rules.

**Stop and ask** only for:

- anything on the hard "no" lists that the task appears to require;
- reopening a locked decision — the test layout, the workspace split, crate
  naming;
- a new third-party dependency;
- a second way to do something a decorator already does;
- a migration that drops or rewrites existing data;
- anything that leaves the machine — a push, a merge to `main`, a tag, a
  publish, a post upstream.

**Progress rule:** progress is measured against the open items of the ask.
After two rounds that do not shrink them, stop and report the blocker with
your proposed decision.

## Keeping these rules small

Rules state decisions; rustdoc beside the code states mechanics and numbers;
`.claude/decisions/` states history. A rule never carries its own history,
never restates code, and never names an item path a reader cannot open.
Budgets: this file 500 lines, `architecture.md` 450, each zone rule 300. A
file at its budget merges or drops a rule before it gains one.

## Reading order

This file plus the **code** are the source of truth.

1. **This file.**
2. **`demo/crates/features/src/users/`** — the reference feature; copy before
   inventing, and fix the exemplar rather than inventing a second pattern.
3. **`demo/apps/api/src/module.rs`** — the canonical composition.

User-level IDE rules (e.g. "explain in French, code and comments in English")
apply per session.

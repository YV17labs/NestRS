---
paths:
  - "**/tests/**/*.rs"
  - "crates/nest-rs-testing/**"
  - "demo/crates/features/src/testing.rs"
---

# Writing tests — the toolbox

A framework crate's tests run in process in `tests/integration/main.rs`, and
against Postgres, Redis or S3 in `tests/e2e/main.rs`, which connects to the dev
container's — always reachable; each suite's modules mirror `src/`. A flat
`tests/<x>.rs` is a binary of its own, relinked per file. This file is the
toolbox and the decisions that keep live tests from meeting each other. Reach
for `nest-rs-testing` before hand-rolling a harness.

## Choosing the level

Every test is paid on every run, so the level is chosen by what must be proved,
cheapest first. Nothing is mocked at any level: what runs is the real code, and
only what the level leaves out is absent.

- **Unit** (`#[cfg(test)] mod tests`) — logic without wiring: a parser, a
  decision table, an error's wording. Systematic: a branch nobody tests is a
  branch nobody knows works.
- **In process** (`TestApp`, `HeadlessApp`) — composition: a decorator's
  expansion booted, a guard on a route, a module's `for_root`. The real DI graph
  and transport run without a socket. The default for framework behaviour, since
  wiring bugs live here.
- **Live service** — only what the service itself decides: the SQL a query
  becomes, a Redis script, a presigned URL, a reconnect. Never a second copy of
  an in-process assertion.
- **Compile-fail snapshot** (trybuild) — only a compile error a developer reads,
  through the umbrella as they read it.
- **Scaffold compile** — only what a generator writes.

Coverage is a map, not a target: it points at the branch nothing executes, and
the test written for it asserts behaviour. Every public contract and every
refusal has a test.

**A slow test is a defect.** The slowest tests set the floor of every run, so a
test that grows is made cheaper first — shared setup, a smaller fixture, the
assertion moved down a level — and what a cheaper level already proves is cut.
nextest prints each test's time: compare before and after.

## `nest-rs-testing`

- **`TestApp` / `TestAppBuilder`** — boots the real DI graph and drives
  HTTP, GraphQL, OpenAPI and MCP through poem's `TestClient`, no socket. The
  default entry point, and the composition witness `manifests-ci.md` asks
  of every `for_root` seam. It boots the transport the app's own
  `HttpModule::for_root(cfg)` describes, through the call the module itself
  makes: pin an `HttpConfig` on the module to test a prefix, a versioning
  strategy, a body cap or a timeout. `TestAppBuilder::http(t)` is only for a
  transport the app does not declare — a bare one tests a server the app
  never runs.
- **`provide` / `provide_arc`** seed a value, which short-circuits its
  resolving factory; **`override_dyn` / `override_value`** swap a provider for
  a double. Never the database (`CLAUDE.md`, hard "no").
- **`HeadlessApp` / `TransportHandle`** — boot with no transport, for
  lifecycle, DI and discovery assertions.
- **`WsApp` / `WsSocket`, `GraphqlSocket`** — a real upgrade, for the edges
  whose protocol is the socket.
- **`LogCapture`** — the events a unit emits are half its contract: a denial
  that fails closed and logs nothing passes every response assertion.
- **`EphemeralDatabase`** (`orm` feature) — a per-test database, dropped with
  the value.
- **`load_project_env`** — the `.env` cascade, so a test reaches the
  devcontainer's `postgres`, `redis` and `rustfs`.

## Live backends are shared — each suite hands out its parts

nextest runs each test in its own process, and both workspaces' suites run on
one Postgres and one Redis. Isolation is declared, never hoped for.

- **A suite that boots over SeaORM runs on an `EphemeralDatabase`**, never on
  the database `<PREFIX>_SEAORM__URL` names: it seeds the connection
  (`provide_arc(db.connection())`), which keeps `SeaOrmModule` from opening
  its pool, and the guard drops the database. A dirty or unmigrated main
  database then fails nothing.
- **Redis's sixteen databases are split between the workspaces.** 0 is the
  developer's — what `nestrs run dev` drains — and the framework suite's
  shared one; the demo holds the lower half above it, the framework the upper
  half. Each list is checked at compile time where it is written: the
  framework's `DB_*` constants in `nest-rs-redis`'s e2e `main.rs`, the demo's
  `features::testing::RedisDatabase`, one variant per test, so two tests on
  one database is a duplicate discriminant. A `FLUSHDB` in one half never
  reaches a job the other filed.
- **A test that starts a worker drains a queue no other test pushes to.** In
  the framework the queue is named for the test. A demo test cannot name its
  queue — `audio` and `notifications` are the product's `#[queue]`s — so its
  own Redis database is the isolation, seeded into every app of the test that
  must meet: the producer and the worker alike. A test that only pushes stays
  on the suites' shared database, where nothing drains.
- **A test needing its own database, user or proxy seeds its config**
  (`TestApp::provide`) on the suite's URL rewritten by `url_on`, `url_as` or
  `url_at`, never a `for_root` pin nor a URL of its own: the suite's own
  `<PREFIX>_REDIS__URL` outranks a pin field by field and would move the app
  back onto the shared database, or around a proxy the test put in front of
  it.
- **What a test files where nothing drains, it names uniquely and deletes.**

## Decisions that bite

- **A test asserts against the shared constant, never a copied literal.** One
  that re-types `"posts:read audio:transcode"` passes while the policy and the
  deployment drift apart; one that reads the constant fails the day they do.
- **A procedure the docs hand an operator is run by a test, as printed** —
  anything that changes data an operator cannot get back.
- **A test waiting on a timer pauses tokio's clock** (`start_paused = true`),
  so a loaded machine moves none of its instants; the scheduler reads its wall
  clock through tokio's, which a paused clock advances. The real clock is left
  to a test that blocks a thread on purpose, and to a live service's clock.
- **nextest is the runner, with no configuration**: `just test` runs every
  test and the doctests.
- **A doc example is compiled, and run unless it serves forever**: a fragment
  carries its scaffolding in hidden `# ` lines, `.run().await` is `no_run`, a
  refusal is `compile_fail`, and `text` is for what is not Rust. Held by review.
- **A doc example compiles with what its crate already depends on**: a
  composition sits with its owner — `#[authorize]` on each edge in
  `nest-rs-authz`, `#[crud]` on `CrudService` — rather than pulling the stack
  into an edge's dev-dependencies, and a macro's examples sit on its surface
  crate's re-export.
- **Builds sharing a target directory take a file lock.** `nest-rs-cli`'s
  scaffold tests compile every generated workspace into
  `target/scaffold-check` and serialize on its `scaffold.lock`; per-test target
  directories are not the fix, each would rebuild the tree. Two builds writing
  one directory show as a linker error on an unrelated crate.
- **A second checkout takes its own `CARGO_TARGET_DIR`.** Sharing one, cargo
  reuses the other checkout's test binaries, whose `CARGO_MANIFEST_DIR` still
  points there, so every test reading the tree reads the wrong one.
  `cargo clean -p <crate>` recovers.
- **trybuild and doctests compile the sources on disk when they run**: a file
  edited during a run voids it. Re-run rather than read its failures.

## Compile-fail snapshots

**One suite holds them, `nest-rs-macro-hygiene`'s**: a folder per umbrella
module, each fixture written through `nest_rs::` as a developer writes it, so a
snapshot pins the words and the span a developer meets. Split across crates,
the suites rebuild each other's dependencies: trybuild runs cargo with the
identity of the crate under test, and ring's build script tracks it
(cargo#16134).

**A snapshot pins the refusal its fixture exists for, and no error the fixture
made on its own.** A fixture that does not parse, or whose names no longer
resolve, stays red whatever the decorator says, so the refusal it promises can
change or vanish with the suite green. A fixture therefore parses and its
`.stderr` carries no name-resolution error, unless its `//!` says
`deliberately does not parse` or `deliberately fails to resolve`. Held by
review, like anything else a regenerated snapshot pins: read the `.stderr`
`TRYBUILD=overwrite` wrote before committing it.

**Snapshots change only on a deliberate toolchain bump**, or where rustc lists
a trait's implementors and the framework gained one. rustc's wording is the
toolchain's, and `rust-toolchain.toml` pins it, so a `.stderr` moves in the
commit that bumps the toolchain; any other snapshot diff is a refusal that
changed. `just test` runs them. `cargo fmt` never
reaches a fixture (no module tree declares it), and reformatting one moves the
line numbers its `.stderr` pins.

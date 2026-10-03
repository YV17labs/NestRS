---
paths:
  - "**/tests/**/*.rs"
  - "crates/nest-rs-testing/**"
  - "demo/crates/features/src/testing.rs"
  - ".config/nextest.toml"
---

# Writing tests — the toolbox

The test layout, the two suite names, the runner and "e2e infra is always
reachable" are `CLAUDE.md`'s, and locked — a flat `tests/<x>.rs` is a binary of
its own, outside the nextest gates and relinked per file. This file is the
toolbox and the decisions that keep live suites from meeting each other. Reach
for `nest-rs-testing` before hand-rolling a harness.

## `nest-rs-testing`

- **`TestApp` / `TestAppBuilder`** — boots the real DI graph and drives
  HTTP, GraphQL, OpenAPI and MCP through poem's `TestClient`, no socket. The
  default e2e entry point, and the composition witness `CLAUDE.md` asks of
  every `for_root` seam. It boots the transport the app's own
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
- **`load_project_env`** — the `.env` cascade, so an e2e reaches the
  devcontainer's `postgres`, `redis` and `rustfs`.

## `cargo mutants` — advisory

A test is evidence for what it asserts, and a surviving mutant shows what it
does not — a stronger question than coverage's *did this line run*. Nothing
here measures tests by name or by count. Mutants are advisory: CI runs them on
the diff and never blocks, and locally they are on demand for a logic change. A
missed mutant gets the test that kills it, or the commit body says why it is
equivalent; an unviable one is noise.

```
git diff HEAD > /tmp/c.diff
CARGO_TARGET_DIR=target/mutants cargo mutants --in-diff /tmp/c.diff \
  --test-tool nextest -p <touched crate> -j1
```

- **Its own target directory.** It builds in a copy of the tree under
  `TMPDIR`, and the trybuild projects it leaves behind point into that
  deleted copy, which breaks the next build of anything else sharing the
  directory.
- **`-j1` whenever the target directory is shared**, since every job then
  builds into it at once. With `CARGO_TARGET_DIR` unset, each job builds in its
  own copy: parallel, slower cold.
- **`--test-tool nextest`**, because the suites are nextest's.

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
- **A test needing its own database or user seeds its config**
  (`TestApp::provide`), never a `for_root` pin: the suite's own
  `<PREFIX>_REDIS__URL` outranks a pin field by field and would move the app
  back onto the shared database, or around a proxy the test put in front of
  it.
- **What a test files where nothing drains, it names uniquely and deletes** —
  unless deleting means writing apalis's keys outside its API (`CLAUDE.md`,
  hard "no"); then it stays.

## Decisions that bite

- **A test asserts against the shared constant, never a copied literal.** One
  that re-types `"posts:read audio:transcode"` passes while the policy and the
  deployment drift apart; one that reads the constant fails the day they do.
- **A procedure the docs hand an operator is run by an e2e test, as printed** —
  a move, a drain, anything that changes data an operator cannot get back.
- **Runner configuration is `.config/nextest.toml`**, read automatically, so
  every invocation in `CLAUDE.md` is the same on every machine.
- **A suite sharing a build directory declares a test group.** `nest-rs-cli`'s
  e2e compiles every scaffolded workspace into one target directory and runs
  in the `scaffold-check` group; the trybuild suites (`diagnostics.rs` in each
  crate's integration suite) run in the `trybuild` group. Both have
  `max-threads = 1`. Per-test target directories are not the fix: each would
  rebuild the tree. A missing group shows as a linker error on an unrelated
  crate, which reads as a broken toolchain.
- **A second checkout takes its own `CARGO_TARGET_DIR`.** Sharing one, cargo
  reuses the other checkout's test binaries, whose `CARGO_MANIFEST_DIR` still
  points there, so every test reading the tree reads the wrong one.
  `cargo clean -p <crate>` recovers.
- **trybuild and doctests compile the sources on disk when they run**: a file
  edited during a run voids it. Re-run rather than read its failures.

## Compile-fail snapshots

**A snapshot pins the refusal its fixture exists for, and no error the fixture
made on its own.** A fixture that does not parse, or whose names no longer
resolve, stays red whatever the decorator says, so the refusal it promises can
change or vanish with the suite green. A fixture therefore parses and its
`.stderr` carries no name-resolution error, unless its `//!` says
`deliberately does not parse` or `deliberately fails to resolve`. Held by
review, like anything else a regenerated snapshot pins: read the `.stderr`
`TRYBUILD=overwrite` wrote before committing it.

**Snapshots change only on a deliberate toolchain bump.** rustc's wording is
the toolchain's, and `rust-toolchain.toml` pins it, so a `.stderr` moves in the
commit that bumps the toolchain and nowhere else; a snapshot diff in any other
commit is a refusal that changed. The local loop leaves the snapshots out
(`!test(/_diagnostics$/)`) unless a `*-macros` or `nest-rs-codegen` crate
moved. The format hook skips `tests/*/diagnostics/`: `cargo fmt` never reaches
a fixture, and reformatting one moves the line numbers its `.stderr` pins.

---
paths:
  - "**/tests/**/*.rs"
  - "crates/nest-rs-testing/**"
  - "demo/crates/features/src/testing.rs"
---

# Writing tests — the toolbox

A framework crate's tests run in process in `tests/integration/main.rs`, and
against Postgres, Valkey or S3 in `tests/e2e/main.rs`, which connects to the dev
container's — always reachable, over TLS alone; each suite's modules mirror
`src/`. A decorator's expansion is tested in its surface crate (`macros.md`),
so its folder there, named for the decorator, mirrors the macro crate's `src/`:
`nest-rs-seaorm`'s `tests/integration/expose/wire.rs` is
`nest-rs-seaorm-macros/src/wire.rs`. A flat `tests/<x>.rs` is a binary of its
own, relinked per file. This
file is the toolbox and the decisions that keep live tests from meeting each
other. Reach for `nest-rs-testing` before hand-rolling a harness.

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

- **`TestApp` / `TestAppBuilder`** — boots the real DI graph and drives HTTP,
  GraphQL, OpenAPI and MCP through nestrs's `TestClient`, the stack the server
  runs, no socket; `TestApp` and the builder's `build` are the `http` feature,
  `build_ws` the `ws` feature, which brings it; the builder itself is neither. The
  default entry point, and the composition witness `manifests-ci.md` asks of
  every `for_root` seam. **A suite boots the module `main` boots**, its
  app-wide layer pools included (`container.md`): an app's suite never
  re-types them, and a framework test declares the pool it runs on a root
  module of its own. It boots the transport the app's own
  `HttpModule::for_root(cfg)` describes, through the call the module itself
  makes: pin an `HttpConfig` on the module to test a prefix, a versioning
  strategy, a body cap or a timeout, or seed the whole config (`provide`) for
  a value the module does not pin — the transport has no builder.
- **`provide` / `provide_arc`** seed a value, which short-circuits its
  resolving factory and still passes its config's checks;
  **`override_dyn` / `override_value` / `override_arc`** replace a provider
  before anything is built from it, so every consumer injects the double.
  Never the database (`CLAUDE.md`, hard "no").
- **`HeadlessApp` / `TransportHandle`** — boot with no transport, for
  lifecycle, DI and discovery assertions. A suite serving no HTTP starts from
  `HeadlessApp::builder()` and leaves `http` off, so it links no HTTP stack.
- **`WsApp` / `WsSocket`, `GraphqlSocket`** — a real upgrade, for the edges
  whose protocol is the socket.
- **`LogCapture`** — the events a unit emits are half its contract: a denial
  that fails closed and logs nothing passes every response assertion.
- **`printed`** — everything a failure prints, however it is read: a test holding
  a marked value asserts it appears in nothing a `{:?}` or a `main` would write.
- **`wait_until` / `wait_for`** — a condition the test cannot await, failing at
  the caller's line past its bound, never a loop of its own.
- **`<subject>_kit!`** (the port's `<subject>` feature) — one test per case
  a driver of that port owes, against the driver's backend through the seam
  in `nest_rs_testing::<subject>`; named for the port (`queue_kit!`), never
  a backend.
- **`TestAuthority`** (`tls` feature, on under the umbrella's `testing`) —
  the certificates a TLS double presents and a client trusts, issued in
  process. A double in front of a service ends
  TLS on both sides — the test authority's certificate to the client, the
  system's trust to the service — so what it reads or holds back is the
  protocol, never a record; the client under test trusts both authorities.
- **`TestAppBuilder::ephemeral_database::<Migrator>()`** (`orm` feature) —
  the one boot on a database of the test's own: created and migrated for the
  test, held by the app and dropped after it; a test seeding rows, or booting a
  second app on it, reads the connection back from the booted app's container.
  `EphemeralDatabase` alone is for a test that boots no app, or whose subject
  is a pool opening its URL.
- **`TestApp::unit(ability, || …)` / `system_unit(|| …)`** (`authz`,
  `database`) — a service's body run in one unit of the app's bound
  `UnitOfWork`, as a request granted the ability, a visitor's too, or as system
  work, committed on `Ok`: never an executor or an ability installed by hand.
- **`TestAppBuilder::ephemeral_signing_key()` / `TestApp::bearer(&claims)`**
  (`authn`) — the app verifies the tokens of a key pair the test holds, issued
  in process as `TestAuthority` issues certificates, in place of its configured
  key material, signing only where that material signs and verifying only when
  it names none; `bearer` signs a claims value for its audience, as its issuer,
  `exp` at its lifetime unless the claims carry one. No app's suite commits a
  private key, and none signs with the app's own
  (`.claude/decisions/suite-signs-with-a-key-of-its-own.md`).
- **`load_project_env`** — the `.env` cascade, so a test reaches the
  devcontainer's `postgres`, `valkey-standalone` and `rustfs`.

## Live backends are shared — each suite hands out its parts

nextest runs each test in its own process, and both workspaces' suites run on
one Postgres, one Redis and one store. Isolation is declared, never hoped for.

- **A suite that boots over SeaORM boots on `ephemeral_database`**, never on
  the database `<PREFIX>_SEAORM__URL` names: the connection seeded keeps
  `SeaOrmModule` from opening its pool, and the app drops the database. A
  dirty or unmigrated main database then fails nothing. `nest-rs-seaorm`'s own
  suite alone also boots on that URL: its subject is the pool and the binding
  over it, and it works on probe tables of its own.
- **Redis's databases are split between development and the suites**, on
  every Valkey the dev container runs, which declares enough for each
  (`--databases`, a Cluster's `cluster-databases`): 0 is the developer's —
  what `nestrs run dev` drains — and no suite files a key there; the demo's
  suites hold a range above it, the framework's the range above theirs, its
  shared tests on a database of their own, the one the URL `test.just` exports
  names, and the suite refuses a URL naming another. Each list is checked at
  compile time where it is written: the framework's `DB_*` constants in
  `nest-rs-redis`'s e2e `main.rs`, below the count the dev container
  declares, and the demo's `features::testing::RedisDatabase`, one variant
  per test, so two tests on one database is a duplicate discriminant. A
  `FLUSHDB` in one range never reaches a job another filed. The demo's range
  bounds its worker tests: one more takes a database past the framework's, or
  a queue-key namespace the queue binding does not have.
- **A test that starts a worker drains a queue no other test pushes to.** In
  the framework the queue is named for the test. A demo test cannot name its
  queue — `audio` and `notifications` are the product's `#[queue]`s — so its
  own Redis database is the isolation, seeded into every app of the test that
  must meet: the producer and the worker alike. A test that only pushes
  leaves no job on Redis: the harness arms the outbox of an app binding a
  store and a queue backend at its boot, as its relay does when served, so
  the job waits in the throwaway database, and a test needing its job run
  spawns `OutboxRelay` beside its worker; a push with no database cancels its
  job by its receipt.
- **The demo's suites file into a bucket of their own**, which its test
  profile names and the dev container creates beside development's, so what a
  test uploads never sits among the objects a running app lists; the
  features' integration suite holds both profiles to
  `features::testing::SUITES_BUCKET`. The framework's storage suite shares
  development's bucket.
- **A test needing its own database, user or proxy seeds its config**
  (`TestApp::provide`) on the suite's URL rewritten by `url_on`, `url_as` or
  `url_at`, never a `for_root` pin nor a URL of its own: the suite's own
  `<PREFIX>_REDIS__URL` outranks a pin field by field and would move the app
  back onto the shared database, or around a proxy the test put in front of
  it.
- **What a test files where nothing drains, it names uniquely and deletes**
  once its assertions are read — an object, a record, a job. A failing test
  leaves no such name either: no later run would reach it, so what removes it
  runs however the test ends (`nest-rs-redis`'s `forgetting`, and
  `forgetting_user` for an ACL user named for its run), or the key expires.
- **`nest-rs-redis`'s e2e runs on the Valkey topology its URL declares**: the
  dev container's one server by default, which CI runs too. With the
  `valkey-sentinel` or `valkey-cluster` dev container open, uncommenting its
  URL in `test.just` points the same `just test` at it, by hand when the
  binding changes; `test.just` reads the topology off the URL
  (`.claude/decisions/one-server-per-backend.md`). A test that holds on
  one topology alone sits in a module named for it (`standalone`, `sentinel`,
  `cluster`), and each run leaves the others out; a failover moves what every
  other test runs on, and a node asked for a client certificate refuses every
  other test's connection, so each sits in a module of its own (`failover`,
  `mutual_tls`), run last and alone, and gives back what it changed.
  A test counting a job's runs boots on the queue's default lease and the
  connection's default budget (`patient_replica_on`), and a lapse it is about it
  forces (`RedisKit`'s `idle`): on the suite's one-second lease a stalled runner
  sees the job run again, which at-least-once allows, so no count is asserted
  across a lapse the test did not force. What follows a worker's call, it reads
  off what Redis holds, never off the worker giving the call up at its lease's
  end; one whose subject is a call failing keeps the suite's budget on that
  default lease, so the call fails and is asked again well inside it (a settle
  whose answer is lost). A renewal alone is waited out, on the
  suite's renewed lease of a few seconds (`LEASE_RENEWED`): a renewal goes a
  third into it, so a stall under the other two thirds lapses nothing. The
  queue kit's cases do the same on `KitLease::Lasting`, through the backend's
  `lapse` and `take`, and the two whose subject is a renewal on
  `KitLease::Brief`. A connection of the suite's
  own opens and is answered within the framework's default budget
  (`suite_connection`, `at_default_budget`), wide of a stall; one telling a
  frozen node apart keeps `redis`'s, or bounds its probe itself.
  An administration command reaches every node it concerns — `CLIENT KILL` ids
  and `ACL` users are per server — never one the client picks.

## Decisions that bite

- **A test asserts against the shared constant, never a copied literal.** One
  that re-types `"posts:read audio:transcode"` passes while the policy and the
  deployment drift apart; one that reads the constant fails the day they do.
- **A procedure the docs hand an operator is run by a test, as printed** —
  anything that changes data an operator cannot get back.
- **A test waiting on a timer pauses tokio's clock** (`start_paused = true`),
  so a loaded machine moves none of its instants; the scheduler reads its wall
  clock through tokio's, which a paused clock advances. The real clock is left
  to a test that blocks a thread on purpose, and to a live service's clock —
  and there no margin on elapsed time is the proof, since a stalled runner
  fails it with the behaviour right: "at once" is the attempts a double
  accepted, "promptly" an order the test holds (a gate it releases, a lock
  still held), a cost the thread's CPU clock. Where only real time tells two
  outcomes apart, the bound is the other outcome's own duration, said beside
  it; any other real-clock wait only guards against a hang.
- **nextest is the runner, with no configuration**: `just test` runs every
  test and the doctests. A `LEAK` mark on macOS is the runner's: std creates a
  child's pipes there in two steps (`pipe`, then `FD_CLOEXEC`), so a test
  process spawned from another thread in between holds another test's stdout
  until it exits. Linux creates them in one (`pipe2`), so a `LEAK` in the
  container or CI is a subprocess the test left.
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

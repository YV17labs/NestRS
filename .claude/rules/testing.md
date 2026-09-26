---
paths:
  - "**/tests/**/*.rs"
  - "crates/nest-rs-testing/**"
---

# Writing tests — the toolbox

The layout/suite norm, the runner and the "e2e infra is always
reachable" rule live in `CLAUDE.md` (locked — don't reopen). This file
is the toolbox: reach for `nest-rs-testing` before hand-rolling a
harness.

## `nest-rs-testing` helpers

- **`TestApp` / `TestAppBuilder`** — boots the real DI graph and drives
  HTTP/GraphQL/OpenAPI/MCP through poem's `TestClient` (re-exported),
  no socket. The default e2e entry point. **It boots the transport the
  app's own `HttpModule::for_root(cfg)` describes**, through
  `HttpTransport::from_config` — the same call the module's
  `TransportContribution` makes. So pin an `HttpConfig` on the module to
  test a non-default prefix, versioning strategy, body cap or timeout;
  `TestAppBuilder::http(t)` is for a transport the app does *not*
  declare. It built a bare transport once, and a suite asserting
  `/widgets` shipped an app serving `/api/widgets`.
- **`override_dyn` / `override_value`** on the builder — swap a
  provider for a test double at build time. Never for the DB —
  mocking the database in e2e is a hard no.
- **`HeadlessApp` / `TransportHandle`** — boot with no transport, for
  lifecycle, DI and discovery assertions.
- **`EphemeralDatabase`** (behind the `orm` feature) — a per-test
  database, dropped with the value.
- **`load_project_env`** — loads the `.env` cascade so e2e picks up
  the devcontainer hostnames (`postgres`, `redis`, `rustfs`).

## What is missing is a cell, not a feeling

Coverage answers *did this line run*. Nothing answers *was this answer
asserted* — a line executed ten times by a green test whose emitted value
nobody read is 100 % covered and wrong. So the unit is neither the test nor
the percentage: **a test is one cell in a matrix whose row is a member of a
family and whose column is an obligation every member owes.**

Everything the framework interprets belongs to a family — the decorators and
their halves, the edges, the layer families, the `for_root` seams, the umbrella
features, the `warn`+ events, the manifests the repo owns *and generates*.
Four clauses, load-bearing in this order.

1. **A family is declared at its second member.** The second thing the
   framework interprets the same way is not a second thing, it is a family.
   Declaring it costs three lines: how its members are **derived from the
   source**, what every member owes, and **how a member is spelled**. Deriving
   is the whole of it — a hand-written member list is the defect, not the
   shortcut: one family here is guarded twice in one file, once from a derived
   population and once from eleven literal paths, and the drift was in the
   literal half.

2. **A declared family is joined, and the join answers both questions.** One
   test per family joins members × obligations. An empty cell **fails** — that
   is the hole. The other direction is not a failure but a **precondition**:
   the join names each cell's occupants, and **a cell that has one is closed —
   no second test is written for it.** A test already there for its own
   scenario stays; what is forbidden is adding one *for coverage*, which is how
   a suite grows without gaining an assertion. The join is **workspace-wide**:
   a member covered from another crate is covered, and a per-crate view
   manufactures false holes that get closed with duplicate tests — one crate
   here was reported untested while its whole public surface was asserted from
   four other crates.

   **A join lands on existing code through a baseline, never through a sprint.**
   A baseline line is existing code, or a name a dependency dictates and this
   repo cannot change — recorded with its upstream issue; code the same change
   writes is never baselined, it is fixed. Today's empty cells are recorded once; the join fails on the *next* one, and
   the baseline **only shrinks** — the docs linter's contract, for the same
   reason. Filling a pre-existing cell is ranked work, not a debt to clear:
   `warn`+ events deciding access come first because they are what an incident
   queries, and a cell whose emptiness is a decision is written down as one.

3. **A filled cell is a proved cell.** A test filling a cell must fail when the
   behaviour it asserts is removed — establish that once, while writing it. A
   green cell that would stay green is worse than an empty one: the matrix
   reads as covered and the join goes quiet.

4. **The spelling is the whole mechanism.** A test covering a member spells that
   member the way the framework spells it — in its file name, its function
   name, or a literal in its body. Nothing else is needed and nothing else
   works: a family whose members cannot be spelled cannot be joined, so the
   spelling is decided when the family is declared, never per test.

**This catches absence, never wrongness.** A cell filled by a test asserting
the wrong thing passes the join; that is what `/audit` is for, and the two do
not substitute for each other.

Two moves in this need judgement and have no grep: noticing that something has
**become** a family, and writing a cell body that would actually fail. Both are
work for an agent; the join itself never is.

**A join reads the tree, never the directories above it.** Every path a join
classifies is read below the repository root — `sources::segments` for its
components, `sources::relative` for its spelling, both through `sources::below`,
which refuses a path outside the tree rather than falling back to the absolute
one. Until 7.0 some joins read absolute components, so a checkout under `~/src/`
or inside a folder named like an edge changed verdicts: under `…/schedule/`, the
framework's guards read as schedule adapters.
`naming::no_verdict_depends_on_where_the_checkout_sits` plants one tree under a
plain root and under `…/src/schedule/nestrs` and asks the path-reading joins and
`nestrs lint` for the same written verdict, and a unit test keeps `.components()`
and `.ancestors()` out of every join.

Before writing any test: **which family is this a member of, what does that
family owe, and how is the member spelled?** A test that answers none of the
three is covering product behaviour, not a framework obligation, and this
section does not bind it.

## Reminders that bite

- The e2e gate is the nextest filter `binary(e2e)` — never `#[ignore]`.
- nextest does not run doctests: `cargo test --doc` is its own step
  (demo's `test unit` recipe runs both).
- A DB/Redis/S3 connection failure in the devcontainer is a regression
  to report, never a reason to skip e2e.
- `nest-rs-testing`'s own test tree organizes by concern — the one
  sanctioned exception to "mirror `src/`".
- **Runner config is `.config/nextest.toml`**, read automatically, so a
  *Definition of done* invocation stays the same everywhere.
- **A suite sharing a build directory declares a test group there.**
  nextest runs each test in its own *process*, so anything shared
  between tests is shared between processes. `nest-rs-cli`'s e2e
  compiles every scaffolded workspace into one `CARGO_TARGET_DIR` —
  worth keeping, it turns minutes into seconds — and concurrent builds
  raced on the fingerprints of shared dependencies. It surfaced as a
  **linker** error on whichever generic crate lost (`quote`,
  `proc-macro2`, `libc`), naming nothing about the cause and moving
  between runs, which reads exactly like a broken toolchain.
  `max-threads = 1` on a group scoped to that binary is the fix;
  per-test target directories are not, since each would rebuild the
  whole tree.
- **A live backend is shared the same way, so an e2e suite hands its parts
  out where it declares them.** A test that needs a Redis logical database
  of its own takes it from the one `DB_*` list in its suite's `main.rs`,
  which a `const` block checks at compile time — every index from 1 to 15,
  no two equal — so a collision is a build error rather than two tests
  flushing each other's keys. A test that starts a worker drains a queue
  named for that test alone, since a worker in one process takes another
  test's jobs; a test that pushes where no worker drains uses a name or a
  key unique to its run, and deletes what it filed.
- **A test that needs its own database or user seeds its config**
  (`TestApp::provide`) rather than pinning a `for_root` base: the suite's
  own `<PREFIX>_REDIS__URL` outranks a pin, field by field, and would move
  the app back onto the shared database — or around the proxy the test put
  in front of it.
- **The events join reads what `syn` parses, and `syn` never parses a
  macro's tokens.** A message spelled inside `assert!` or `assert_eq!`
  reads as unasserted, however the test fares. Bind the line first —
  `let line = logs.expect_one(TARGET, "…");` — then assert on the binding.
- **A procedure the docs hand an operator is run by an e2e test**, as
  printed: a move, a drain, anything that changes data an operator cannot
  get back. The 6.x queue move is one
  (`layout::a_queue_moved_out_of_the_6x_layout_runs_every_job_it_held_once`),
  and like any filled cell it is proved: leaving out the `ZADD` that
  re-registers the moved in-flight set fails it.
- **A second checkout takes its own `CARGO_TARGET_DIR`.** Sharing one, cargo
  reuses the other checkout's test binaries, whose `CARGO_MANIFEST_DIR`
  still points there — so every conformance join reads the other tree, or,
  once that tree is gone, an empty one whose floors fail within
  milliseconds. `cargo clean -p nest-rs-conformance -p nest-rs-cli` recovers.
- **trybuild and doctests compile the sources on disk when they run**, so a
  file edited while a suite runs voids that run; re-run it rather than
  reading its failures.

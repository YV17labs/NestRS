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
   Today's empty cells are recorded once; the join fails on the *next* one, and
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

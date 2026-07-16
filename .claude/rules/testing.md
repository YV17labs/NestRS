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
  no socket. The default e2e entry point.
- **`override_dyn` / `override_value`** on the builder — swap a
  provider for a test double at build time. Never for the DB —
  mocking the database in e2e is a hard no.
- **`HeadlessApp` / `TransportHandle`** — boot with no transport, for
  lifecycle, DI and discovery assertions.
- **`EphemeralDatabase`** (behind the `orm` feature) — a per-test
  database, dropped with the value.
- **`load_project_env`** — loads the `.env` cascade so e2e picks up
  the devcontainer hostnames (`postgres`, `redis`, `rustfs`).

## Reminders that bite

- The e2e gate is the nextest filter `binary(e2e)` — never `#[ignore]`.
- nextest does not run doctests: `cargo test --doc` is its own step
  (demo's `test unit` recipe runs both).
- A DB/Redis/S3 connection failure in the devcontainer is a regression
  to report, never a reason to skip e2e.
- `nest-rs-testing`'s own test tree organizes by concern — the one
  sanctioned exception to "mirror `src/`".

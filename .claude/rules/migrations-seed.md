---
paths:
  - "demo/crates/migrations/**"
  - "demo/crates/seed/**"
---

# Migrations & seed — demo persistence ops

Both crates are product-side (`demo/`); the framework never depends on
them. Drive them via `nestrs run db <up|down|fresh|status|seed|reset>`
from `demo/`.

## Migrations (`demo/crates/migrations`)

- **File name = `m<YYYYMMDD>_<NNNNNN>_<desc>.rs`** — date + zero-padded
  ordinal for same-day ordering (`m20260526_000001_create_user.rs`).
- Each file holds `#[derive(DeriveMigrationName)] pub struct Migration`
  with `up`/`down`. **Register in two places, in chronological order:**
  the `mod` line in `lib.rs` and the `migrations()` vec in
  `migrator.rs`. Miss either and the migration silently never runs.
- House column pattern: `created_at`/`updated_at` defaulting to
  `current_timestamp`; nullable `deleted_at` for soft delete.
- A migration that drops or rewrites existing data is an **owner
  decision** (see *Autonomous work* in `CLAUDE.md`) — stop and ask.

## Seed (`demo/crates/seed`)

- One factory per entity in `factories/<entity>.rs`; `runner.rs` calls
  them in FK-dependency order (org → user → post). A new factory =
  the file + a `factories/mod.rs` line + a `runner.rs` call at the
  right position in that order.
- **Deterministic and idempotent by design**: fixed
  `Uuid::from_u128(…)` constants (stable ids across resets) and
  `ON CONFLICT DO NOTHING` inserts written with raw `sea_query` — not
  the entities. Each factory returns `rows_affected`; the runner sums
  them. Keep both properties when adding data — a random id or a plain
  insert breaks `db seed` re-runs.

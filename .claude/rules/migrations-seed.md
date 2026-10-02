---
paths:
  - "demo/crates/migrations/**"
  - "demo/crates/seed/**"
---

# Migrations and seed — demo persistence

Both crates are the product's; the framework never depends on them. Drive them
from `demo/` with `nestrs run db <recipe>`.

## Migrations (`demo/crates/migrations`)

- **A file is `m<YYYYMMDD>_<NNNNNN>_<desc>.rs`** — the date, then a
  zero-padded ordinal for same-day order.
- Each holds `#[derive(DeriveMigrationName)] pub struct Migration` with `up`
  and `down`, and is **registered twice, in chronological order**: its `mod`
  line in `lib.rs` and its entry in `migrator.rs`'s `migrations()`. Missing the
  second, it never runs.
- **House columns:** `created_at` / `updated_at` defaulting to
  `current_timestamp`; a nullable `deleted_at` for soft delete.

## Seed (`demo/crates/seed`)

- **One factory per entity** in `factories/<entity>.rs`, called by
  `runner.rs` in foreign-key order — a parent before what references it. A new
  factory is its file, its `factories/mod.rs` line and its `runner.rs` call at
  its place in that order.
- **Deterministic and idempotent:** fixed `Uuid::from_u128(…)` ids, stable
  across resets, and `ON CONFLICT DO NOTHING` inserts written with `sea_query`
  rather than the entities. Each factory returns its affected rows and the
  runner sums them. A random id or a plain insert breaks a second
  `db seed`.

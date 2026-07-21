//! Shared Postgres connection for the e2e suite — one place for the env-var
//! contract instead of a copy per module.
//!
//! Same shape as the sibling live-backend suites (`nest-rs-redis`'s
//! `redis_url`, `nest-rs-storage`'s `StorageConfig::default`): the dev
//! container's address is the default, and `NESTRS_DATABASE__URL` overrides it
//! to point at a Postgres outside the container. The framework workspace
//! deliberately ships no `.env` (that is the product's, under `demo/`), so a
//! hard `expect` on the variable made the whole suite unrunnable from the
//! workspace-wide `-E 'binary(e2e)'` step.

use std::sync::Arc;

use sea_orm::{ConnectionTrait, Database, DatabaseConnection};

/// The dev container's Postgres, wired by `.devcontainer` and mirrored in
/// `demo/.env`.
const DEFAULT_URL: &str = "postgres://nestrs:nestrs@postgres:5432/nestrs";

fn url() -> String {
    std::env::var("NESTRS_DATABASE__URL").unwrap_or_else(|_| DEFAULT_URL.to_owned())
}

pub(crate) async fn connect() -> DatabaseConnection {
    let url = url();
    Database::connect(&url)
        .await
        .unwrap_or_else(|err| panic!("connect to Postgres at {url}: {err}"))
}

pub(crate) async fn connect_arc() -> Arc<DatabaseConnection> {
    Arc::new(connect().await)
}

/// Run the one-time DDL (+ seed) for a probe table shared by several tests.
///
/// nextest gives **each test its own process**, so a `OnceCell` guard only
/// serializes within one of them — and `CREATE TABLE IF NOT EXISTS` races the
/// Postgres catalog between processes, which fails the whole batch on a fresh
/// database. Serialize on a transaction-level advisory lock instead: it is held
/// by whichever process gets there first and released at `COMMIT`, so the
/// others wait and then find the table already there.
///
/// The lock key is derived from `table`, so two probe tables cannot collide and
/// no caller has to invent a magic number. `sql` must be `;`-terminated
/// statements that are safe to re-run (`IF NOT EXISTS`, `ON CONFLICT DO
/// NOTHING`).
pub(crate) async fn setup_shared_table(conn: &DatabaseConnection, table: &str, sql: &str) {
    let lock_key = advisory_lock_key(table);
    conn.execute_unprepared(&format!(
        "BEGIN; SELECT pg_advisory_xact_lock({lock_key}); {sql} COMMIT;"
    ))
    .await
    .unwrap_or_else(|err| panic!("set up the shared probe table `{table}`: {err}"));
}

/// FNV-1a over the table name — a stable `i64` that does not depend on the
/// std hasher's per-process seed (advisory locks must agree *across* nextest
/// processes, so `DefaultHasher` would be wrong here).
fn advisory_lock_key(table: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in table.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash as i64
}

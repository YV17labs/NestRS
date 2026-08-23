//! Shared Postgres connection for the e2e suite — one place for the env-var
//! contract instead of a copy per module.
//!
//! Same shape as the sibling live-backend suites (`nest-rs-redis`'s
//! `redis_url`, `nest-rs-storage`'s `StorageConfig::default`): the dev
//! container's address is the default, and `NESTRS_SEAORM__URL` overrides it
//! to point at a Postgres outside the container. The framework workspace
//! deliberately ships no `.env` (that is the product's, under `demo/`), so a
//! hard `expect` on the variable made the whole suite unrunnable from the
//! workspace-wide `-E 'binary(e2e)'` step.

use std::sync::Arc;

use sea_orm::{ConnectionTrait, Database, DatabaseConnection};

/// The dev container's Postgres, wired by `.devcontainer` and mirrored in
/// `demo/.env`.
const DEFAULT_URL: &str = "postgres://nestrs:nestrs@postgres:5432/nestrs";

pub(crate) fn url() -> String {
    std::env::var(nest_rs_config::var_name("seaorm", "URL"))
        .unwrap_or_else(|_| DEFAULT_URL.to_owned())
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

/// The two tables a commit-time failure is provoked with: a child row whose
/// foreign key is `DEFERRABLE INITIALLY DEFERRED`, so inserting it against a
/// parent that never arrives succeeds and the `COMMIT` is what refuses.
///
/// Three suites need it — the HTTP boundary, the worker's per-attempt
/// transaction and the WS/MCP data context — for the same reason each time, and
/// they differ only in the prefix that keeps their tables apart. Shared because
/// the shape *is* the assertion: a probe that stopped deferring would make all
/// three green while testing nothing, and the drift would be invisible in each
/// file on its own. Returns the child table's name, which is what the caller
/// inserts into.
pub(crate) async fn deferred_probe_tables(conn: &DatabaseConnection, prefix: &str) -> String {
    let parents = format!("{prefix}_parents");
    let children = format!("{prefix}_children");
    setup_shared_table(
        conn,
        &children,
        &format!(
            "CREATE TABLE IF NOT EXISTS {parents} (id INT PRIMARY KEY);
             CREATE TABLE IF NOT EXISTS {children} (
                 id INT PRIMARY KEY,
                 parent_id INT NOT NULL REFERENCES {parents}(id)
                     DEFERRABLE INITIALLY DEFERRED
             );"
        ),
    )
    .await;
    children
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

/// A pool of exactly one connection, with a short acquire timeout — the shape a
/// saturated pool or a restarted database actually presents.
///
/// The caller holds that one connection (`TransactionTrait::begin`) for the
/// length of the probe; everything else then fails to acquire. Two suites build
/// this, and their timeouts had already drifted apart, which is what a shared
/// fixture is for.
pub(crate) async fn starved_pool() -> DatabaseConnection {
    let mut options = sea_orm::ConnectOptions::new(url());
    options
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_millis(250));
    sea_orm::Database::connect(options)
        .await
        .expect("a one-connection pool connects")
}

/// The backend pid serving `executor`, so a probe can have the server close that
/// exact session out from under it.
pub(crate) async fn backend_pid(executor: &impl ConnectionTrait) -> i32 {
    executor
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT pg_backend_pid()",
        ))
        .await
        .expect("read this session's backend pid")
        .expect("one row")
        .try_get_by_index(0)
        .expect("an i32 pid")
}

/// Terminate `pid` from another connection — `57P01` on whatever that session
/// was doing.
pub(crate) async fn terminate_backend(killer: &DatabaseConnection, pid: i32) {
    killer
        .execute_unprepared(&format!("SELECT pg_terminate_backend({pid})"))
        .await
        .expect("terminate the probe's backend");
}

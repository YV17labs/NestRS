//! Shared Postgres connection for the suite, over TLS alone: the URL is
//! `<PREFIX>_SEAORM__URL`, which the dev container and CI set, and every
//! connection is opened with the pool's own options, so it verifies the
//! server's certificate as the app does.

use std::sync::Arc;

use sea_orm::{ConnectionTrait, Database, DatabaseConnection};

pub(crate) fn url() -> String {
    nest_rs_config::ConfigService::for_namespace("seaorm")
        .get("URL")
        .expect("a readable database URL")
        .expect("the dev container and CI name the suite's Postgres")
}

/// The pool's own options for `url`.
pub(crate) fn options(url: String) -> sea_orm::ConnectOptions {
    nest_rs_seaorm::SeaOrmConfig {
        url,
        ..nest_rs_seaorm::SeaOrmConfig::default()
    }
    .connect_options()
}

pub(crate) async fn connect() -> DatabaseConnection {
    Database::connect(options(url()))
        .await
        .unwrap_or_else(|err| panic!("connect to the suite's Postgres: {err}"))
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
    let mut options = options(url());
    options
        .max_connections(1)
        .min_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_lazy(true)
        .acquire_timeout(std::time::Duration::from_millis(250));
    let pool = sea_orm::Database::connect(options)
        .await
        .expect("a one-connection pool is built");
    // sqlx counts opening a connection against the acquire timeout, which a
    // loaded machine can spend on the handshake alone; a lazy pool with neither
    // an idle timeout nor a lifetime opens its minimum in the background instead,
    // under sqlx's own deadline.
    nest_rs_testing::wait_until(std::time::Duration::from_secs(10), || {
        pool.get_postgres_connection_pool().num_idle() == 1
    })
    .await;
    pool
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

/// What a piece of after-commit work saw when it ran.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Sighting {
    /// The probe table's rows, counted on a connection **outside** the
    /// boundary — so only what had committed.
    pub(crate) committed: i64,
    /// Whether the work's own ambient executor was the pool, outside the
    /// transaction it waited for.
    pub(crate) on_pool: bool,
}

/// Where the probe's work records what it saw — empty while it has not run.
pub(crate) type Sightings = Arc<std::sync::Mutex<Vec<Sighting>>>;

/// A fresh one-column table for one test, so a row count is that test's alone.
pub(crate) async fn after_commit_table(conn: &DatabaseConnection, table: &str) {
    for sql in [
        format!("DROP TABLE IF EXISTS {table}"),
        format!("CREATE TABLE {table} (id INT PRIMARY KEY)"),
    ] {
        conn.execute_unprepared(&sql)
            .await
            .unwrap_or_else(|err| panic!("set up the probe table `{table}`: {err}"));
    }
}

/// `table`'s rows on `conn`.
pub(crate) async fn committed_rows(conn: &DatabaseConnection, table: &str) -> i64 {
    conn.query_one_raw(sea_orm::Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        format!("SELECT count(*)::bigint AS n FROM {table}"),
    ))
    .await
    .expect("the count runs")
    .expect("a count returns a row")
    .try_get::<i64>("", "n")
    .expect("the count is an integer")
}

/// Run from inside a boundary's body: write one row on the ambient executor,
/// then hand the boundary work for after its commit that records what it sees.
///
/// Shared by every settle site — the HTTP boundary, the worker's attempt, the
/// WS/MCP data context — because the probe *is* the assertion. The work counts
/// on `observer`, a connection outside the boundary, so a sighting of `1`
/// proves the row had committed when it ran: work run at once would have counted
/// `0` there, while counting on the boundary's own executor would have seen its
/// uncommitted row and passed either way.
pub(crate) async fn write_and_hold(
    table: &'static str,
    observer: Arc<DatabaseConnection>,
    sightings: Sightings,
) {
    nest_rs_seaorm::current_executor()
        .expect("the body runs with an ambient executor")
        .execute_unprepared(&format!("INSERT INTO {table} (id) VALUES (1)"))
        .await
        .expect("the write lands in the boundary's transaction");
    nest_rs_database::after_commit(async move {
        let committed = committed_rows(&observer, table).await;
        let on_pool = matches!(
            nest_rs_seaorm::current_executor(),
            Some(nest_rs_seaorm::Executor::Pool(_))
        );
        sightings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Sighting { committed, on_pool });
    })
    .await;
}

/// What the probe's work recorded.
pub(crate) fn sighted(sightings: &Sightings) -> Vec<Sighting> {
    std::mem::take(
        &mut *sightings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

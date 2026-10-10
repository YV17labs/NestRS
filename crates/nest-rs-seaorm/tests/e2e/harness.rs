//! The suite's Postgres, over TLS alone at `<PREFIX>_SEAORM__URL`, opened with
//! the pool's own options so it verifies the certificate as the app does.

use std::sync::Arc;

use sea_orm::{ConnectionTrait, Database, DatabaseConnection};

pub(crate) fn url() -> String {
    nest_rs_config::ConfigService::for_namespace("seaorm")
        .get("URL")
        .expect("a readable database URL")
        .expect("the dev container and CI name the suite's Postgres")
}

pub(crate) fn options(url: String) -> sea_orm::ConnectOptions {
    nest_rs_seaorm::SeaOrmConfig {
        url,
        ..nest_rs_seaorm::SeaOrmConfig::default()
    }
    .connect_options()
    .expect("the suite's Postgres URL verifies TLS")
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
/// nextest gives each test its own process, and `CREATE TABLE IF NOT EXISTS`
/// races the Postgres catalog between them: the DDL runs under an advisory lock
/// keyed on `table`. `sql` must be `;`-terminated and safe to re-run.
pub(crate) async fn setup_shared_table(conn: &DatabaseConnection, table: &str, sql: &str) {
    let lock_key = advisory_lock_key(table);
    conn.execute_unprepared(&format!(
        "BEGIN; SELECT pg_advisory_xact_lock({lock_key}); {sql} COMMIT;"
    ))
    .await
    .unwrap_or_else(|err| panic!("set up the shared probe table `{table}`: {err}"));
}

/// The two tables a commit-time failure is provoked with: a child whose foreign
/// key is `DEFERRABLE INITIALLY DEFERRED`, so the `COMMIT` is what refuses an
/// orphan. Returns the child table's name.
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

/// FNV-1a over the table name: the lock must agree across nextest processes,
/// which `DefaultHasher`'s per-process seed breaks.
fn advisory_lock_key(table: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in table.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash as i64
}

/// A pool of exactly one connection with a short acquire timeout; the caller
/// holds it (`TransactionTrait::begin`) so every other acquire fails.
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
    // sqlx counts opening a connection against the acquire timeout; a lazy pool
    // opens its minimum in the background, under sqlx's own deadline.
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
/// The work counts on `observer`, outside the boundary: the boundary's own
/// executor would see its uncommitted row and pass either way.
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

//! A throwaway Postgres database fixture for e2e tests (the `orm` feature).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::MigratorTrait;

/// A fresh Postgres database created for a single e2e run: brought up to the
/// current schema with the app's own `Migrator`, then **dropped when this guard
/// is dropped**. The whole point is to need no faking — `DatabaseModule` connects
/// at boot, so the test seeds this real connection and the module's factory is
/// short-circuited (a seed wins over a `for_root` factory of the same type):
///
/// ```ignore
/// let db = EphemeralDatabase::create::<db::Migrator>().await?;
/// let app = TestApp::builder()
///     .module::<AppModule>()
///     .provide_arc(db.connection()) // seeds Arc<DatabaseConnection>; factory skipped
///     .build()
///     .await?;
/// // ... assertions ...
/// // `db` drops here → DROP DATABASE
/// ```
///
/// The base/admin URL comes from `NESTRS_DATABASE__URL` (the devcontainer provides a
/// reachable Postgres). Each run uses a uniquely named `nestrs_e2e_*` database;
/// any left behind by a crashed run are reaped at the next [`create`](Self::create).
pub struct EphemeralDatabase {
    admin_url: String,
    name: String,
    url: String,
    connection: Arc<DatabaseConnection>,
}

impl EphemeralDatabase {
    /// Create and migrate a throwaway database, taking the base URL from
    /// `NESTRS_DATABASE__URL`. Generic over the app's `Migrator` so the schema matches
    /// production exactly.
    pub async fn create<M: MigratorTrait>() -> Result<Self> {
        let admin_url = std::env::var("NESTRS_DATABASE__URL").map_err(|_| {
            anyhow!("NESTRS_DATABASE__URL must point at a reachable Postgres for e2e")
        })?;
        Self::create_with::<M>(&admin_url).await
    }

    /// As [`create`](Self::create) but with an explicit base URL — connect to it,
    /// create a fresh database alongside it, migrate, and return the guard.
    pub async fn create_with<M: MigratorTrait>(admin_url: &str) -> Result<Self> {
        let admin = Database::connect(admin_url).await?;
        let name = unique_name();

        // `CREATE DATABASE` reads `template1`; two running concurrently fail with
        // "source database template1 is being accessed by other users". Tests run
        // in parallel, so serialise creation (cheap — migration runs unlocked).
        {
            let _guard = CREATE_LOCK.lock().await;
            reap_stale(&admin).await;
            admin
                .execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
                .await?;
        }

        let url = swap_database(admin_url, &name);
        let connection = Database::connect(&url).await?;
        M::up(&connection, None).await?;

        Ok(Self {
            admin_url: admin_url.to_owned(),
            name,
            url,
            connection: Arc::new(connection),
        })
    }

    /// The connection to seed into a `TestApp` (`provide_arc(db.connection())`).
    pub fn connection(&self) -> Arc<DatabaseConnection> {
        self.connection.clone()
    }

    /// The throwaway database's URL, for a test that boots a second connection.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for EphemeralDatabase {
    fn drop(&mut self) {
        // `DROP DATABASE` needs an async connection, but `drop` is sync — run it
        // on a dedicated current-thread runtime so teardown works whatever the
        // test's runtime flavour, and blocks until the drop completes. WITH
        // (FORCE) terminates any pool connection still held elsewhere.
        let admin_url = std::mem::take(&mut self.admin_url);
        let name = std::mem::take(&mut self.name);
        let _ = std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                if let Ok(admin) = Database::connect(&admin_url).await {
                    let _ = admin
                        .execute_unprepared(&format!(
                            "DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"
                        ))
                        .await;
                }
            });
        })
        .join();
    }
}

/// Serialises `CREATE DATABASE` across concurrent tests in one process — they
/// would otherwise contend on the `template1` source database.
static CREATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Five minutes, in nanoseconds — the age past which a throwaway database is
/// considered an orphan from a crashed run rather than one in use right now.
const STALE_AFTER_NANOS: u128 = 5 * 60 * 1_000_000_000;

/// Reap throwaway databases orphaned by previous (crashed) runs. Best-effort,
/// and crucially **age-gated**: only databases older than [`STALE_AFTER_NANOS`]
/// are dropped, so a sibling test running concurrently (its database freshly
/// created) is never reaped out from under it.
async fn reap_stale(admin: &DatabaseConnection) {
    let stmt = Statement::from_string(
        DbBackend::Postgres,
        "SELECT datname FROM pg_database WHERE datname LIKE 'nestrs_e2e_%'".to_owned(),
    );
    let Ok(rows) = admin.query_all_raw(stmt).await else {
        return;
    };
    let now = now_nanos();
    for row in rows {
        let Ok(name) = row.try_get::<String>("", "datname") else {
            continue;
        };
        // Name is `nestrs_e2e_<pid>_<nanos>`; an unparseable tail is an unknown
        // (old) format, so treat it as stale.
        // Name is `nestrs_e2e_<pid>_<nanos>_<seq>`: the creation timestamp is the
        // 4th `_`-separated field. An unexpected shape is an unknown (old) format,
        // so treat it as stale.
        let stale = match name.split('_').nth(3).and_then(|t| t.parse::<u128>().ok()) {
            Some(created) => now.saturating_sub(created) > STALE_AFTER_NANOS,
            None => true,
        };
        if stale {
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn unique_name() -> String {
    // A process-wide counter guarantees uniqueness even when two concurrent
    // callers read the same coarse-resolution timestamp (the cause of a flaky
    // "database already exists"). Kept as `<pid>_<nanos>_<seq>` so the reaper can
    // still recover the creation time.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("nestrs_e2e_{}_{}_{}", std::process::id(), now_nanos(), seq)
}

/// Swap the database name in a Postgres URL, preserving any query string.
fn swap_database(url: &str, db: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (url, None),
    };
    let prefix = base.rsplit_once('/').map(|(p, _)| p).unwrap_or(base);
    match query {
        Some(q) => format!("{prefix}/{db}?{q}"),
        None => format!("{prefix}/{db}"),
    }
}

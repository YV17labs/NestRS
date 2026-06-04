//! A throwaway Postgres database fixture for e2e tests (the `orm` feature).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::MigratorTrait;

/// Fresh Postgres database created for one e2e run, migrated, then **dropped
/// when this guard drops**. Seed `db.connection()` into a `TestApp` and the
/// real connection short-circuits `DatabaseModule`'s `for_root` factory.
///
/// Each run uses a unique `nestrs_e2e_*` name; orphans from crashed runs are
/// reaped (age-gated) on the next [`create`](Self::create). Admin URL comes
/// from `NESTRS_DATABASE__URL`.
pub struct EphemeralDatabase {
    admin_url: String,
    name: String,
    url: String,
    connection: Arc<DatabaseConnection>,
}

impl EphemeralDatabase {
    pub async fn create<M: MigratorTrait>() -> Result<Self> {
        let admin_url = std::env::var("NESTRS_DATABASE__URL").map_err(|_| {
            anyhow!("NESTRS_DATABASE__URL must point at a reachable Postgres for e2e")
        })?;
        Self::create_with::<M>(&admin_url).await
    }

    pub async fn create_with<M: MigratorTrait>(admin_url: &str) -> Result<Self> {
        let admin = Database::connect(admin_url).await?;
        let name = unique_name();

        // `CREATE DATABASE` reads `template1`; concurrent CREATEs fail with
        // "source database template1 is being accessed by other users", so
        // serialise creation (cheap — migration runs unlocked).
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

    pub fn connection(&self) -> Arc<DatabaseConnection> {
        self.connection.clone()
    }

    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for EphemeralDatabase {
    fn drop(&mut self) {
        // `DROP DATABASE` is async but `drop` is sync — run on a dedicated
        // current-thread runtime, blocking until done, so teardown works
        // whatever the test runtime flavour. WITH (FORCE) terminates any
        // pool connection still held elsewhere.
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

static CREATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Five minutes — past this a `nestrs_e2e_*` database is an orphan, not in
/// use by a concurrent sibling.
const STALE_AFTER_NANOS: u128 = 5 * 60 * 1_000_000_000;

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
        // Name is `nestrs_e2e_<pid>_<nanos>_<seq>`; an unexpected shape is an
        // older (unknown) format, treated as stale.
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
    // Process-wide counter for uniqueness even when two callers read the same
    // coarse-resolution timestamp; reaper still recovers the time from nanos.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("nestrs_e2e_{}_{}_{}", std::process::id(), now_nanos(), seq)
}

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

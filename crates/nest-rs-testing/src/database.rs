//! A throwaway Postgres database fixture for e2e tests (the `orm` feature).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::MigratorTrait;

use crate::env::load_project_env;

/// Fresh Postgres database created for one e2e run, migrated, then **dropped
/// when this guard drops**. Seed `db.connection()` into a `TestApp` and the
/// real connection short-circuits `SeaOrmDatabaseModule`'s `for_root` factory.
///
/// Each run uses a unique `nest_rs_e2e_*` name; orphans from crashed runs are
/// reaped (age-gated) on the next [`create`](Self::create). Admin URL comes
/// from `<PREFIX>_SEAORM__URL`.
pub struct EphemeralDatabase {
    admin_url: String,
    name: String,
    url: String,
    connection: Arc<DatabaseConnection>,
}

impl EphemeralDatabase {
    /// Create and migrate a fresh database, taking the admin URL from
    /// `<PREFIX>_SEAORM__URL` (loading the project `.env` first). The usual
    /// entry point; errors if the URL is unset.
    pub async fn create<M: MigratorTrait>() -> Result<Self> {
        // The admin URL is read before any `App` boots, so load `.env` first.
        load_project_env();
        let var = nest_rs_config::var_name("seaorm", "URL");
        let admin_url = std::env::var(&var).map_err(|_| {
            anyhow!(
                "{var} is unset and no `.env` was found above the test's working \
                 directory — point it at a reachable Postgres for e2e"
            )
        })?;
        Self::create_with::<M>(&admin_url).await
    }

    /// Create and migrate a fresh database against an explicit admin URL, for
    /// callers that resolve the connection string themselves rather than via
    /// the environment.
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

    /// The live connection to the ephemeral database — seed this into a
    /// [`TestApp`] to short-circuit `SeaOrmDatabaseModule`'s `for_root` factory.
    pub fn connection(&self) -> Arc<DatabaseConnection> {
        self.connection.clone()
    }

    /// The full connection URL of the ephemeral database (admin URL with the
    /// database name swapped in), for callers wiring their own pool.
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

/// Five minutes — past this a [`PREFIX`]`*` database is an orphan, not in use
/// by a concurrent sibling.
const STALE_AFTER_NANOS: u128 = 5 * 60 * 1_000_000_000;

/// The namespace every ephemeral database is created under, and the one the
/// reaper sweeps.
///
/// A constant because two sites *interpret* it and must agree: `unique_name`
/// writes it, `reap_stale` matches it, and `created_nanos` reads a timestamp
/// out of what follows it. Spelled apart, a rename that looks cosmetic
/// silently either strands every orphan forever or — if the new spelling holds
/// a different number of `_` — makes the reaper misread the timestamp,
/// classify every database as stale, and `DROP` a concurrently running
/// sibling's live database.
const PREFIX: &str = "nest_rs_e2e";

async fn reap_stale(admin: &DatabaseConnection) {
    let stmt = Statement::from_string(
        DbBackend::Postgres,
        format!("SELECT datname FROM pg_database WHERE datname LIKE '{PREFIX}\\_%'"),
    );
    let Ok(rows) = admin.query_all_raw(stmt).await else {
        return;
    };
    let now = now_nanos();
    for row in rows {
        let Ok(name) = row.try_get::<String>("", "datname") else {
            continue;
        };
        // An unexpected shape is an older (unknown) format, treated as stale.
        let stale = match created_nanos(&name) {
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

/// The `<nanos>` of a [`unique_name`], read by stripping the prefix rather than
/// by counting `_` across it — the count was derived by hand and a rename would
/// have silently shifted it.
fn created_nanos(name: &str) -> Option<u128> {
    name.strip_prefix(PREFIX)?
        .split('_')
        .nth(2)?
        .parse::<u128>()
        .ok()
}

fn now_nanos() -> u128 {
    // A clock before the epoch is not a thing a test fixture should panic over.
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

fn unique_name() -> String {
    // Process-wide counter for uniqueness even when two callers read the same
    // coarse-resolution timestamp; reaper still recovers the time from nanos.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{PREFIX}_{}_{}_{}", std::process::id(), now_nanos(), seq)
}

/// The admin URL with its database name replaced.
///
/// RFC 3986 §3.2: the authority is introduced by `//` and terminated by the
/// next `/`, `?` or `#` — so the path starts at the *first* slash after the
/// scheme, never the last. Splitting on the last one dropped the host of any
/// path-less URL (`postgres://host:5432` yielded `postgres://<db>`), surfacing
/// much later as a connection error naming a URL the developer never wrote.
fn swap_database(url: &str, db: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (url, None),
    };
    let authority_at = base.find("//").map_or(0, |i| i + 2);
    let prefix = match base[authority_at..].find('/') {
        Some(offset) => &base[..authority_at + offset],
        None => base,
    };
    match query {
        Some(q) => format!("{prefix}/{db}?{q}"),
        None => format!("{prefix}/{db}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swapping_the_database_keeps_the_authority() {
        assert_eq!(
            swap_database("postgres://u:p@host:5432/postgres", "tmp"),
            "postgres://u:p@host:5432/tmp",
        );
        // The regression: no path at all. The last `/` is the second one of
        // `//`, so the host used to vanish.
        assert_eq!(
            swap_database("postgres://host:5432", "tmp"),
            "postgres://host:5432/tmp",
        );
        assert_eq!(
            swap_database("postgres://host/postgres?sslmode=require", "tmp"),
            "postgres://host/tmp?sslmode=require",
        );
    }

    #[test]
    fn a_name_round_trips_through_the_reaper() {
        let name = unique_name();
        assert!(name.starts_with(PREFIX));
        assert!(created_nanos(&name).is_some_and(|n| n > 0));
        assert_eq!(created_nanos("nest_rs_e2e_nope"), None);
    }
}

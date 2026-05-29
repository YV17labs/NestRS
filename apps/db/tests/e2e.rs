//! End-to-end for the shared-database tooling against a **real, throwaway
//! Postgres**: migrate a fresh database with this crate's own [`Migrator`], run the
//! seed, and confirm the seed is idempotent (re-running inserts nothing). `db` has
//! no `AppModule` to boot — its surface *is* the migrations + seed — so its e2e
//! exercises exactly that.
//!
//! Requires a reachable Postgres at `DATABASE_URL` (the devcontainer provides one).

use db::Migrator;
use nestrs_testing::EphemeralDatabase;

#[tokio::test]
async fn migrate_then_seed_populates_and_is_idempotent() {
    let database = EphemeralDatabase::create::<Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let conn = database.connection();

    // The first seed brings in the demo rows.
    let inserted = db::seed::run(conn.as_ref()).await.expect("seed runs");
    assert!(inserted > 0, "the first seed inserts demo rows");

    // Re-running is safe and inserts nothing — the idempotence the seed promises.
    let again = db::seed::run(conn.as_ref()).await.expect("seed re-runs");
    assert_eq!(again, 0, "a second seed is idempotent");
}

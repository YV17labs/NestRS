use migrations::Migrator;
use nest_rs_testing::EphemeralDatabase;

#[tokio::test]
async fn migrate_then_seed_populates_and_is_idempotent() {
    let database = EphemeralDatabase::create::<Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let conn = database.connection();

    let inserted = seed::run(conn.as_ref()).await.expect("seed runs");
    assert!(inserted > 0, "the first seed inserts demo rows");

    let again = seed::run(conn.as_ref()).await.expect("seed re-runs");
    assert_eq!(again, 0, "a second seed is idempotent");
}

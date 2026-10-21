//! [`OutboxMigration`] — the framework's migration creating the outbox's
//! table. A later change to it ships as a migration of its own, never as an
//! edit of this one, which deployed databases recorded as applied.

use sea_orm::{ConnectionTrait, DbErr};
use sea_orm_migration::{MigrationName, MigrationTrait, SchemaManager, async_trait};

/// Creates `nestrs_outbox`, the table [`SeaOrmOutbox`](super::SeaOrmOutbox)
/// keeps a unit's jobs in until the relay files them; registered in an app's
/// migrator beside its own migrations, by `nestrs g migration nestrs_outbox`.
///
/// ```
/// use nest_rs_seaorm::queue::OutboxMigration;
/// use sea_orm_migration::{MigrationName, MigrationTrait, MigratorTrait};
///
/// pub struct Migrator;
///
/// impl MigratorTrait for Migrator {
///     fn migrations() -> Vec<Box<dyn MigrationTrait>> {
///         vec![Box::new(OutboxMigration)]
///     }
/// }
///
/// assert_eq!(OutboxMigration.name(), "m20261009_000001_nestrs_outbox");
/// ```
pub struct OutboxMigration;

impl MigrationName for OutboxMigration {
    fn name(&self) -> &str {
        "m20261009_000001_nestrs_outbox"
    }
}

/// The envelope is the queue port's wire format, kept as text and read back by
/// the port alone; the database's clock decides when a record is due.
const UP: &str = "CREATE TABLE nestrs_outbox (
    seq bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    queue text NOT NULL,
    job_id uuid NOT NULL UNIQUE,
    sealed text NOT NULL,
    unique_key text NULL,
    due_at timestamptz NOT NULL,
    written_at timestamptz NOT NULL DEFAULT now(),
    attempts integer NOT NULL DEFAULT 0,
    next_attempt_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX nestrs_outbox_next ON nestrs_outbox (next_attempt_at, seq);";

#[async_trait::async_trait]
impl MigrationTrait for OutboxMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(UP)
            .await
            .map(drop)
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE nestrs_outbox")
            .await
            .map(drop)
    }
}

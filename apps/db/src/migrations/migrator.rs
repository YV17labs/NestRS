//! The migration runner — enumerates and orders the schema migrations for SeaORM,
//! which tracks the applied ones in `seaql_migrations`.

use sea_orm_migration::prelude::*;

use super::{m20260526_000000_create_org, m20260526_000001_create_user};

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260526_000000_create_org::Migration),
            Box::new(m20260526_000001_create_user::Migration),
        ]
    }
}

/// Apply every pending migration to `conn` — the programmatic form of the
/// `migrate up` binary. Lets a test harness bring a throwaway database up to the
/// current schema before booting an app against it.
pub async fn migrate(conn: &sea_orm::DatabaseConnection) -> anyhow::Result<()> {
    Migrator::up(conn, None).await?;
    Ok(())
}

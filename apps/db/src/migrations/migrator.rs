use sea_orm_migration::prelude::*;

use super::{
    m20260526_000000_create_org, m20260526_000001_create_user, m20260526_000002_add_user_role,
};

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260526_000000_create_org::Migration),
            Box::new(m20260526_000001_create_user::Migration),
            Box::new(m20260526_000002_add_user_role::Migration),
        ]
    }
}

pub async fn migrate(conn: &sea_orm::DatabaseConnection) -> anyhow::Result<()> {
    Migrator::up(conn, None).await?;
    Ok(())
}

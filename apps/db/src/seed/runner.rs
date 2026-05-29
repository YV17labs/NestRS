//! Drives the per-entity factories in foreign-key order.

use anyhow::Result;
use sea_orm::DatabaseConnection;

use super::factories;

/// Seeds the demo data and returns the number of rows actually inserted (0 when
/// everything already exists — re-running, or running after `migrate fresh`, is
/// safe). Orgs are seeded before users so the `user.org_id` foreign key resolves.
pub async fn run(db: &DatabaseConnection) -> Result<u64> {
    let mut inserted = 0;
    inserted += factories::org::seed(db).await?;
    inserted += factories::user::seed(db).await?;
    Ok(inserted)
}

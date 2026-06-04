use anyhow::Result;
use sea_orm::DatabaseConnection;

use super::factories;

pub async fn run(db: &DatabaseConnection) -> Result<u64> {
    let mut inserted = 0;
    inserted += factories::org::seed(db).await?;
    inserted += factories::user::seed(db).await?;
    Ok(inserted)
}

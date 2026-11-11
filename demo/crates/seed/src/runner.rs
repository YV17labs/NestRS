use nest_rs::seaorm::RepoError;
use sea_orm::DatabaseConnection;

use super::factories;

pub async fn run(db: &DatabaseConnection) -> Result<u64, RepoError> {
    let mut inserted = 0;
    inserted += factories::org::seed(db).await?;
    inserted += factories::user::seed(db).await?;
    inserted += factories::post::seed(db).await?;
    Ok(inserted)
}

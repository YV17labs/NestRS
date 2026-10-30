use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "user_identity" ALTER COLUMN "id" SET DEFAULT uuidv7();
                   ALTER TABLE "post_publication" ALTER COLUMN "id" SET DEFAULT uuidv7();"#,
            )
            .await
            .map(drop)
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "user_identity" ALTER COLUMN "id" DROP DEFAULT;
                   ALTER TABLE "post_publication" ALTER COLUMN "id" DROP DEFAULT;"#,
            )
            .await
            .map(drop)
    }
}

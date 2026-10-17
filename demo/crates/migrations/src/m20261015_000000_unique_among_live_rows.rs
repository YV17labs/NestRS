use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "user" DROP CONSTRAINT "user_email_key";
                   ALTER TABLE "org" DROP CONSTRAINT "org_name_key";"#,
            )
            .await?;
        manager
            .create_index(live_unique(
                "user_email_live",
                User::Table,
                User::Email,
                User::DeletedAt,
            ))
            .await?;
        manager
            .create_index(live_unique(
                "org_name_live",
                Org::Table,
                Org::Name,
                Org::DeletedAt,
            ))
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(Index::drop().name("user_email_live").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("org_name_live").to_owned())
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "user" ADD CONSTRAINT "user_email_key" UNIQUE ("email");
                   ALTER TABLE "org" ADD CONSTRAINT "org_name_key" UNIQUE ("name");"#,
            )
            .await
            .map(drop)
    }
}

fn live_unique(
    name: &str,
    table: impl IntoIden,
    column: impl IntoIden,
    deleted_at: impl IntoIden + 'static,
) -> IndexCreateStatement {
    Index::create()
        .name(name)
        .table(table)
        .col(column)
        .unique()
        .and_where(Expr::col(deleted_at).is_null())
        .to_owned()
}

#[derive(DeriveIden)]
enum User {
    Table,
    Email,
    DeletedAt,
}

#[derive(DeriveIden)]
enum Org {
    Table,
    Name,
    DeletedAt,
}

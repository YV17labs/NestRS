use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UserIdentity::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UserIdentity::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(UserIdentity::UserId).uuid().not_null())
                    .col(ColumnDef::new(UserIdentity::Provider).string().not_null())
                    .col(ColumnDef::new(UserIdentity::Subject).string().not_null())
                    .col(ColumnDef::new(UserIdentity::Email).string().null())
                    .col(
                        ColumnDef::new(UserIdentity::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(UserIdentity::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .index(
                        Index::create()
                            .name("uq_user_identity_provider_subject")
                            .col(UserIdentity::Provider)
                            .col(UserIdentity::Subject)
                            .unique(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_identity_user_id")
                            .from(UserIdentity::Table, UserIdentity::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UserIdentity::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum UserIdentity {
    Table,
    Id,
    UserId,
    Provider,
    Subject,
    Email,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
}

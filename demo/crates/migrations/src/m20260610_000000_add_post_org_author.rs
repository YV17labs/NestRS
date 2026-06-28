use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Post::Table)
                    .add_column(ColumnDef::new(Post::OrgId).uuid().not_null())
                    .add_column(ColumnDef::new(Post::AuthorId).uuid().not_null())
                    .add_foreign_key(
                        TableForeignKey::new()
                            .name("fk_post_org_id")
                            .from_tbl(Post::Table)
                            .from_col(Post::OrgId)
                            .to_tbl(Org::Table)
                            .to_col(Org::Id)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .add_foreign_key(
                        TableForeignKey::new()
                            .name("fk_post_author_id")
                            .from_tbl(Post::Table)
                            .from_col(Post::AuthorId)
                            .to_tbl(User::Table)
                            .to_col(User::Id)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Post::Table)
                    .drop_foreign_key(Alias::new("fk_post_author_id"))
                    .drop_foreign_key(Alias::new("fk_post_org_id"))
                    .drop_column(Post::AuthorId)
                    .drop_column(Post::OrgId)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Post {
    Table,
    OrgId,
    AuthorId,
}

#[derive(DeriveIden)]
enum Org {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
}

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PostPublication::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PostPublication::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(PostPublication::PostId)
                            .uuid()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(PostPublication::ActorId).uuid().not_null())
                    .col(
                        ColumnDef::new(PostPublication::PublishedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_post_publication_post_id")
                            .from(PostPublication::Table, PostPublication::PostId)
                            .to(Post::Table, Post::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PostPublication::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PostPublication {
    Table,
    Id,
    PostId,
    ActorId,
    PublishedAt,
}

#[derive(DeriveIden)]
enum Post {
    Table,
    Id,
}

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Org::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Org::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Org::Name).string().not_null().unique_key())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Org::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Org {
    Table,
    Id,
    Name,
}

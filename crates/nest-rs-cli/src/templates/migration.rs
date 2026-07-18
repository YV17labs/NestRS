//! **Migration** template — a SeaORM migration skeleton (`g migration`).
//!
//! A create-table starting point with the house columns
//! (`created_at`/`updated_at`/`deleted_at`); edit it for an alter instead. The
//! generator also registers it in `lib.rs` and regenerates `migrator.rs`, so
//! the migration actually runs — the registration you forget is the one that
//! silently never applies.

pub const MIGRATION: &str = r#"use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Create-table skeleton — replace with an `alter_table` for a change.
        manager
            .create_table(
                Table::create()
                    .table({{pascal}}::Table)
                    .if_not_exists()
                    .col(ColumnDef::new({{pascal}}::Id).uuid().not_null().primary_key())
                    // TODO: add your columns here.
                    .col(
                        ColumnDef::new({{pascal}}::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new({{pascal}}::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new({{pascal}}::DeletedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table({{pascal}}::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum {{pascal}} {
    Table,
    Id,
    CreatedAt,
    UpdatedAt,
    DeletedAt,
}
"#;

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Mirrors `apps/api/src/users/entity.rs`: a client-generated UUID v7
        // primary key (no auto-increment), a non-null org scope, and a unique email.
        // `org_id` carries a foreign key to `org` (created by the prior migration) —
        // the DB-level mirror of the SeaORM `belongs_to`/`has_many` relation.
        manager
            .create_table(
                Table::create()
                    .table(User::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(User::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(User::OrgId).uuid().not_null())
                    .col(ColumnDef::new(User::Name).string().not_null())
                    .col(ColumnDef::new(User::Email).string().not_null().unique_key())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_org_id")
                            .from(User::Table, User::OrgId)
                            .to(Org::Table, Org::Id)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(User::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
    OrgId,
    Name,
    Email,
}

// Referenced only by the `org_id` foreign key; the table itself is owned by the
// `create_org` migration.
#[derive(DeriveIden)]
enum Org {
    Table,
    Id,
}

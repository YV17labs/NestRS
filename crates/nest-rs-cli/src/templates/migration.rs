//! **Migration** templates — the `migrations` + `seed` crates every workspace
//! ships, and the SeaORM migration skeleton `g migration` writes into them.
//!
//! [`MIGRATION`] is a create-table starting point with the house columns
//! (`created_at`/`updated_at`/`deleted_at`); edit it for an alter instead. The
//! generator also registers it in `lib.rs` and regenerates `migrator.rs`, so
//! the migration actually runs — the registration you forget is the one that
//! silently never applies.
//!
//! The two crates are scaffolded by `nestrs new`, because `db.just` — shipped
//! in the same breath — names them in every recipe: `nestrs run db up` on a
//! fresh workspace has to apply zero migrations, not fail on a missing package.

pub const CRATE_CARGO: &str = r#"[package]
name = "migrations"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
anyhow.workspace = true
nest-rs.workspace = true
sea-orm.workspace = true
sea-orm-migration.workspace = true
tokio.workspace = true
tracing-subscriber.workspace = true
"#;

// `lib.rs` and `migrator.rs` are not consts: both are *derived* from the
// migration module list (`render_lib` / `render_migrator` in the generator), so
// a fresh crate and one with eight migrations come out of the same code path.

/// The binary behind every `nestrs run db <verb>`. `connect_from_env` is the
/// single connector for tools outside the DI container: it resolves
/// `NESTRS_DATABASE__*` through the same `.env` cascade the apps use, so a tool
/// and its app can never disagree about which database they mean.
pub const CRATE_BIN: &str = r#"use anyhow::{Context, Result, bail};
use migrations::Migrator;
use sea_orm_migration::MigratorTrait;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("NESTRS_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let conn = nest_rs::seaorm::connect_from_env().await?;
    match std::env::args().nth(1).as_deref() {
        Some("up") => Migrator::up(&conn, None).await?,
        Some("down") => {
            let steps: u32 = match std::env::args().nth(2) {
                Some(arg) => arg.parse().context("steps must be a positive integer")?,
                None => 1,
            };
            Migrator::down(&conn, Some(steps)).await?
        }
        Some("fresh") => Migrator::fresh(&conn).await?,
        Some("refresh") => Migrator::refresh(&conn).await?,
        Some("reset") => Migrator::reset(&conn).await?,
        Some("status") => Migrator::status(&conn).await?,
        other => bail!("usage: migrate <up|down [N]|fresh|refresh|reset|status> (got {other:?})"),
    }
    Ok(())
}
"#;

pub const SEED_CARGO: &str = r#"[package]
name = "seed"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
anyhow.workspace = true
nest-rs.workspace = true
sea-orm.workspace = true
tokio.workspace = true
"#;

/// `nestrs run db seed`. Empty, but connected: the wiring a fixture needs is
/// already here, so adding one is a body edit rather than a new crate.
pub const SEED_BIN: &str = r#"//! Demo/reference data, applied by `nestrs run db seed`.
//!
//! `nestrs run db reset` runs `fresh` and then this, and you will run it again
//! on a database that already has rows — so every insert here must be
//! idempotent (find-or-create, or `ON CONFLICT DO NOTHING`).

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let conn = nest_rs::seaorm::connect_from_env().await?;
    conn.ping().await?;

    // Insert your fixtures here.
    println!("seed: nothing to insert yet");
    Ok(())
}
"#;

/// The create-table skeleton. Render it through the **table**'s [`Names`]
/// ([`crate::naming::migration_subject`]), never the migration's — the enum
/// reaches the DDL verbatim.
///
/// [`Names`]: crate::naming::Names
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
                    .table({{singular}}::Table)
                    .if_not_exists()
                    .col(ColumnDef::new({{singular}}::Id).uuid().not_null().primary_key())
                    // TODO: add your columns here, and as a variant of the enum below.
                    .col(
                        ColumnDef::new({{singular}}::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new({{singular}}::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new({{singular}}::DeletedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table({{singular}}::Table).to_owned())
            .await
    }
}

// `DeriveIden` snake-cases these into SQL: this enum writes the
// `{{table}}` table and its columns. Rename it if your entity's
// `#[sea_orm(table_name = "…")]` says otherwise — the two must agree.
#[derive(DeriveIden)]
enum {{singular}} {
    Table,
    Id,
    CreatedAt,
    UpdatedAt,
    DeletedAt,
}
"#;

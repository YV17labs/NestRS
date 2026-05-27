//! `migrate` binary: apply or roll back the shared database schema.
//!
//! SeaORM's `cli::run_cli` reads `DATABASE_URL` and parses the
//! up/down/fresh/status/reset subcommands. Ships in the container image as
//! `/usr/local/bin/migrate`; locally use `just db up` / `just db fresh`.
use sea_orm_migration::prelude::*;

#[tokio::main]
async fn main() {
    cli::run_cli(db::Migrator).await;
}

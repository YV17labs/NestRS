use anyhow::{Context, Result, bail};
use migrations::Migrator;
use sea_orm_migration::MigratorTrait;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let conn = nest_rs_seaorm::connect_from_env().await?;
    match std::env::args().nth(1).as_deref() {
        Some("up") => Migrator::up(&conn, None).await?,
        // Revert one migration by default; `migrate down N` reverts the last N.
        // Never `None` here — that reverts *every* migration and drops the
        // schema. Use `reset` for the full teardown.
        Some("down") => {
            // A provided `steps` argument that isn't a valid integer is a hard
            // error — never silently fall back to reverting a single migration.
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

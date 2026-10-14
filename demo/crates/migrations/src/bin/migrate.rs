use anyhow::{Context, Result, bail};
use migrations::Migrator;
use nest_rs::core::logging::{LogSettings, install_console};
use sea_orm_migration::MigratorTrait;

#[nest_rs::main]
async fn main() -> Result<()> {
    install_console(&LogSettings::from_env()?);

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

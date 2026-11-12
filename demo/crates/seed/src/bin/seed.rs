use anyhow::Result;
use nest_rs::core::logging::{LogSettings, install_console};

#[nest_rs::main]
async fn main() -> Result<()> {
    install_console(&LogSettings::from_env()?);

    let conn = nest_rs::seaorm::connect_from_env().await?;
    let inserted = seed::run(&conn).await?;
    tracing::info!(inserted, "seed complete");
    Ok(())
}

use anyhow::Result;
use nest_rs::core::logging::{ConsoleWriter, LogSettings};
use tracing_subscriber::EnvFilter;

#[nest_rs::main]
async fn main() -> Result<()> {
    let log = LogSettings::from_env()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&log.filter))
        .with_writer(ConsoleWriter::get())
        .init();

    let conn = nest_rs::seaorm::connect_from_env().await?;
    let inserted = seed::run(&conn).await?;
    tracing::info!(inserted, "seed complete");
    Ok(())
}

use anyhow::Result;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let conn = nest_rs_seaorm::connect_from_env().await?;
    let inserted = seed::run(&conn).await?;
    tracing::info!(inserted, "seed complete");
    Ok(())
}

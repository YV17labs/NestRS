use anyhow::Result;
use sea_orm::Database;

#[tokio::main]
async fn main() -> Result<()> {
    let url = std::env::var("NESTRS_DATABASE__URL")
        .map_err(|_| anyhow::anyhow!("NESTRS_DATABASE__URL must be set"))?;
    let conn = Database::connect(url).await?;
    let inserted = db::seed::run(&conn).await?;
    println!("seed: {inserted} row(s) inserted");
    Ok(())
}

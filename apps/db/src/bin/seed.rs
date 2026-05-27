//! `seed` binary: populate the shared database with demo data.
//!
//! Connects using `DATABASE_URL` and delegates to [`db::seed::run`]. Built by
//! the same `cargo build --workspace --bins` the Dockerfile runs, so it ships
//! in the image as `/usr/local/bin/seed` (run with `docker run … /usr/local/bin/seed`).
//! Locally: `just db seed`.
use anyhow::Result;
use sea_orm::Database;

#[tokio::main]
async fn main() -> Result<()> {
    let url =
        std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL must be set"))?;
    let conn = Database::connect(url).await?;
    let inserted = db::seed::run(&conn).await?;
    println!("seed: {inserted} row(s) inserted");
    Ok(())
}

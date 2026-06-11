use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let conn = nest_rs_seaorm::connect_from_env().await?;
    let inserted = seed::run(&conn).await?;
    println!("seed: {inserted} row(s) inserted");
    Ok(())
}

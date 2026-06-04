use sea_orm_migration::prelude::*;

#[tokio::main]
async fn main() {
    if let Ok(url) = std::env::var("NESTRS_DATABASE__URL") {
        std::env::set_var("DATABASE_URL", url);
    }
    cli::run_cli(migrations::Migrator).await;
}

use sea_orm_migration::prelude::*;

#[tokio::main]
async fn main() {
    // `sea_orm_migration`'s `run_cli` reads the connection URL from `DATABASE_URL`,
    // a name it hardcodes. The framework's user-facing variable is the namespaced
    // `NESTRS_DATABASE__URL`, so bridge it across for the third-party CLI rather
    // than expose a second name.
    if let Ok(url) = std::env::var("NESTRS_DATABASE__URL") {
        std::env::set_var("DATABASE_URL", url);
    }
    cli::run_cli(db::Migrator).await;
}

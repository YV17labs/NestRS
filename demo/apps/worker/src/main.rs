use anyhow::Result;
use nest_rs::config::Environment;
use nest_rs::core::App;

use worker::WorkerModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();

    App::builder()
        .module::<WorkerModule>()
        .build()
        .await?
        .run()
        .await
}

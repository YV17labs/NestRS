use anyhow::Result;
use nest_rs::core::App;

use worker::WorkerModule;

#[nest_rs::main]
async fn main() -> Result<()> {
    App::builder()
        .module::<WorkerModule>()
        .build()
        .await?
        .run()
        .await
}

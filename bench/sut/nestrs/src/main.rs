use anyhow::Result;
use nest_rs::config::Environment;
use nest_rs::core::App;

use sut_nestrs::SutModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();

    App::builder()
        .module::<SutModule>()
        .build()
        .await?
        .run()
        .await
}

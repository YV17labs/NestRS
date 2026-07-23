use anyhow::Result;
use nest_rs_config::Environment;
use nest_rs_core::App;

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

use anyhow::Result;
use nest_rs::config::Environment;
use nest_rs::core::App;

use assistant::AssistantModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();

    App::builder()
        .module::<AssistantModule>()
        .build()
        .await?
        .run()
        .await
}

use anyhow::Result;
use nest_rs::config::Environment;
use nest_rs::core::App;

use assistant::AssistantModule;

#[nest_rs::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();

    App::builder()
        .module::<AssistantModule>()
        .build()
        .await?
        .run()
        .await
}

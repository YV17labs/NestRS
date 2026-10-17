use anyhow::Result;
use nest_rs::core::App;

use assistant::AssistantModule;

#[nest_rs::main]
async fn main() -> Result<()> {
    App::builder()
        .module::<AssistantModule>()
        .build()
        .await?
        .run()
        .await
}

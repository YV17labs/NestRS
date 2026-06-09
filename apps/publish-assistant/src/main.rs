use anyhow::Result;
use nest_rs_core::App;
use nest_rs_opentelemetry::OpenTelemetry;

use publish_assistant::PublishAssistantModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _opentelemetry = OpenTelemetry::init("publish-assistant")?;

    App::builder()
        .module::<PublishAssistantModule>()
        .build()
        .await?
        .run()
        .await
}

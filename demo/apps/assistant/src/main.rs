use anyhow::Result;
use nest_rs::core::App;
use nest_rs::opentelemetry::OpenTelemetry;

use assistant::AssistantModule;

#[nest_rs::main]
async fn main() -> Result<()> {
    let _otel = OpenTelemetry::init()?;

    App::builder()
        .module::<AssistantModule>()
        .build()
        .await?
        .run()
        .await
}

use anyhow::Result;
use nest_rs_config::Environment;
use nest_rs_core::App;
use nest_rs_opentelemetry::OpenTelemetry;

use assistant::AssistantModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();
    let _opentelemetry = OpenTelemetry::init("assistant")?;

    App::builder()
        .module::<AssistantModule>()
        .build()
        .await?
        .run()
        .await
}

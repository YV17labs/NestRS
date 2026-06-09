use anyhow::Result;
use nest_rs_config::Environment;
use nest_rs_core::App;
use nest_rs_opentelemetry::OpenTelemetry;

use publish_worker::PublishWorkerModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();
    let _opentelemetry = OpenTelemetry::init("publish-worker")?;

    App::builder()
        .module::<PublishWorkerModule>()
        .build()
        .await?
        .run()
        .await
}

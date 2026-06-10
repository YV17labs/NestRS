use anyhow::Result;
use nest_rs_config::Environment;
use nest_rs_core::App;
use nest_rs_opentelemetry::OpenTelemetry;

use worker::WorkerModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();
    let _opentelemetry = OpenTelemetry::init("worker")?;

    App::builder()
        .module::<WorkerModule>()
        .build()
        .await?
        .run()
        .await
}

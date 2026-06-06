use anyhow::Result;
use nest_rs_config::Environment;
use nest_rs_core::App;
use nest_rs_opentelemetry::OpenTelemetry;

use platform_worker::PlatformWorkerModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();
    let _opentelemetry = OpenTelemetry::init("platform-worker")?;

    App::builder()
        .module::<PlatformWorkerModule>()
        .build()
        .await?
        .run()
        .await
}

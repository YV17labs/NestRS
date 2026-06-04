use anyhow::Result;
use nestrs_config::Environment;
use nestrs_core::App;
use nestrs_opentelemetry::OpenTelemetry;

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

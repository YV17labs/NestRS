use anyhow::Result;
use nestrs_config::Environment;
use nestrs_core::App;
use nestrs_queue::QueueWorker;
use nestrs_telemetry::Telemetry;

use platform_worker::AppModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();
    let _telemetry = Telemetry::init("worker")?;

    App::builder()
        .module::<AppModule>()
        .build()
        .await?
        .transport(QueueWorker::new())
        .run()
        .await
}

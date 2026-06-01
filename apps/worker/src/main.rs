use anyhow::Result;
use nestrs_core::App;
use nestrs_queue::QueueWorker;
use nestrs_schedule::Scheduler;
use nestrs_telemetry::Telemetry;

use worker::AppModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = nestrs_config::bootstrap_env();
    let _telemetry = Telemetry::init("worker")?;

    App::builder()
        .module::<AppModule>()
        .build()
        .await?
        .transport(Scheduler::new())
        .transport(QueueWorker::new())
        .run()
        .await
}

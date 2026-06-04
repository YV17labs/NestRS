use anyhow::Result;
use nestrs_config::Environment;
use nestrs_core::App;
use nestrs_http::HttpTransport;
use nestrs_schedule::Scheduler;
use nestrs_telemetry::Telemetry;

use platform_api::AppModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();
    let _telemetry = Telemetry::init("api")?;

    App::builder()
        .module::<AppModule>()
        .build()
        .await?
        .transport(HttpTransport::new().bind("0.0.0.0:3002"))
        .transport(Scheduler::new())
        .run()
        .await
}

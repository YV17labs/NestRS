use anyhow::Result;
use nestrs_core::App;
use nestrs_http::HttpTransport;
use nestrs_telemetry::Telemetry;

use api::AppModule;

#[tokio::main]
async fn main() -> Result<()> {
    // Load the `.env` cascade first, so `Telemetry::init` (which reads the
    // environment before the app is built) sees it. `ConfigModule::for_root`
    // re-runs this idempotently for the DI graph.
    let _environment = nestrs_config::bootstrap_env();
    let _telemetry = Telemetry::init("api")?;

    App::builder()
        .module::<AppModule>()
        .build()
        .await?
        .transport(HttpTransport::new().bind("0.0.0.0:3003"))
        .run()
        .await
}

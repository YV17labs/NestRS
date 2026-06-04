use anyhow::Result;
use nestrs_config::Environment;
use nestrs_core::App;
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
        .run()
        .await
}

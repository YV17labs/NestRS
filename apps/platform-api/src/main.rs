use anyhow::Result;
use nestrs_config::Environment;
use nestrs_core::App;
use nestrs_opentelemetry::OpenTelemetry;

use platform_api::PlatformApiModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();
    let _opentelemetry = OpenTelemetry::init("platform-api")?;

    App::builder()
        .module::<PlatformApiModule>()
        .build()
        .await?
        .run()
        .await
}

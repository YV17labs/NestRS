use anyhow::Result;
use nest_rs_config::Environment;
use nest_rs_core::App;
use nest_rs_opentelemetry::OpenTelemetry;

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

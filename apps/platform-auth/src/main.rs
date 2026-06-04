use anyhow::Result;
use nestrs_config::Environment;
use nestrs_core::App;
use nestrs_opentelemetry::OpenTelemetry;

use platform_auth::PlatformAuthModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();
    let _opentelemetry = OpenTelemetry::init("platform-auth")?;

    App::builder()
        .module::<PlatformAuthModule>()
        .build()
        .await?
        .run()
        .await
}

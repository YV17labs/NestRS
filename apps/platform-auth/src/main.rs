use anyhow::Result;
use nest_rs_config::Environment;
use nest_rs_core::App;
use nest_rs_opentelemetry::OpenTelemetry;

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

use anyhow::Result;
use nest_rs::core::App;
use nest_rs::opentelemetry::OpenTelemetry;

use auth::AuthModule;

#[nest_rs::main]
async fn main() -> Result<()> {
    let _otel = OpenTelemetry::init("auth")?;

    App::builder()
        .module::<AuthModule>()
        .build()
        .await?
        .run()
        .await
}

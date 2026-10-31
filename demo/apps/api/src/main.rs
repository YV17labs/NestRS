use anyhow::Result;
use nest_rs::core::App;
use nest_rs::opentelemetry::OpenTelemetry;

use api::ApiModule;

#[nest_rs::main]
async fn main() -> Result<()> {
    let _otel = OpenTelemetry::init()?;

    App::builder()
        .module::<ApiModule>()
        .build()
        .await?
        .run()
        .await
}

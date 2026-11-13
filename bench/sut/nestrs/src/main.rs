use anyhow::Result;
use nest_rs::core::App;
use nest_rs::opentelemetry::OpenTelemetry;

use sut_nestrs::SutModule;

#[nest_rs::main]
async fn main() -> Result<()> {
    let _otel = OpenTelemetry::init()?;

    App::builder()
        .module::<SutModule>()
        .build()
        .await?
        .run()
        .await
}

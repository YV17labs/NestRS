use anyhow::Result;
use nest_rs_config::Environment;
use nest_rs_core::App;
use nest_rs_opentelemetry::OpenTelemetry;

use sut_nestrs::SutModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();
    let _telemetry = OpenTelemetry::init("sut-nestrs")?;

    App::builder()
        .module::<SutModule>()
        .build()
        .await?
        .run()
        .await
}

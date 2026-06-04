use anyhow::Result;
use nestrs_core::App;
use nestrs_opentelemetry::OpenTelemetry;

use chat::ChatModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _opentelemetry = OpenTelemetry::init("chat")?;

    App::builder()
        .module::<ChatModule>()
        .build()
        .await?
        .run()
        .await
}

use anyhow::Result;
use nest_rs_core::App;
use nest_rs_opentelemetry::OpenTelemetry;

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

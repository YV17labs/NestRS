use anyhow::Result;
use nestrs_core::App;
use nestrs_telemetry::Telemetry;

use mcp::AppModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _telemetry = Telemetry::init("mcp")?;

    App::builder()
        .module::<AppModule>()
        .build()
        .await?
        .run()
        .await
}

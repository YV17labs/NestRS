use anyhow::Result;
use nestrs_core::App;
use nestrs_opentelemetry::OpenTelemetry;

use mcp::McpModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _opentelemetry = OpenTelemetry::init("mcp")?;

    App::builder()
        .module::<McpModule>()
        .build()
        .await?
        .run()
        .await
}

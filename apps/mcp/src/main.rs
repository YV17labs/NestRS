use anyhow::Result;
use nest_rs_core::App;
use nest_rs_opentelemetry::OpenTelemetry;

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

use anyhow::Result;
use nestrs_core::App;
use nestrs_telemetry::Telemetry;

use chat::AppModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _telemetry = Telemetry::init("chat")?;

    App::builder()
        .module::<AppModule>()
        .build()
        .await?
        .run()
        .await
}

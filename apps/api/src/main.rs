use anyhow::Result;
use nestrs_core::App;
use nestrs_http::HttpTransport;
use nestrs_telemetry::Telemetry;

use api::AppModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _telemetry = Telemetry::init("api")?;

    App::builder()
        .module::<AppModule>()
        .build()
        .await?
        .transport(HttpTransport::new().bind("0.0.0.0:3003"))
        .run()
        .await
}

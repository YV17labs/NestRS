use anyhow::Result;
use nestrs_core::App;
use nestrs_http::HttpTransport;
use nestrs_telemetry::Telemetry;

use chat::AppModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _telemetry = Telemetry::init("chat")?;

    App::new::<AppModule>()?
        .transport(HttpTransport::new().bind("0.0.0.0:3005"))
        .run()
        .await
}

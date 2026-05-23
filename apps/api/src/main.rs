mod app;
mod users;

use anyhow::Result;
use nestrs_core::App;
use nestrs_http::HttpTransport;
use nestrs_telemetry::Telemetry;

#[tokio::main]
async fn main() -> Result<()> {
    let _telemetry = Telemetry::init("api")?;

    App::new::<app::AppModule>()
        .transport(HttpTransport::new().bind("0.0.0.0:3002"))
        .run()
        .await
}

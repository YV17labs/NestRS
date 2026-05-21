mod app;
mod hello;

use anyhow::Result;
use nestrs_core::App;
use nestrs_http::HttpTransport;

#[tokio::main]
async fn main() -> Result<()> {
    App::new::<app::AppModule>()
        .transport(HttpTransport::new().bind("0.0.0.0:3001"))
        .run()
        .await
}

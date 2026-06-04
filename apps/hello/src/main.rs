use anyhow::Result;
use nestrs_core::App;
use nestrs_http::HttpTransport;

use hello::AppModule;

#[tokio::main]
async fn main() -> Result<()> {
    App::new::<AppModule>()?
        .transport(HttpTransport::new().bind("0.0.0.0:3000"))
        .run()
        .await
}

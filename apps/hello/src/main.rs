use anyhow::Result;
use nestrs_core::App;

use hello::AppModule;

#[tokio::main]
async fn main() -> Result<()> {
    App::builder()
        .module::<AppModule>()
        .build()
        .await?
        .run()
        .await
}

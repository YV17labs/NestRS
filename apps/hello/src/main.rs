use anyhow::Result;
use nestrs_core::App;

use hello::HelloModule;

#[tokio::main]
async fn main() -> Result<()> {
    App::builder()
        .module::<HelloModule>()
        .build()
        .await?
        .run()
        .await
}

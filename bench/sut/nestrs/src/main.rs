use anyhow::Result;
use nest_rs::core::App;

use sut_nestrs::SutModule;

#[nest_rs::main]
async fn main() -> Result<()> {
    App::builder()
        .module::<SutModule>()
        .build()
        .await?
        .run()
        .await
}

use anyhow::Result;
use features::authn::AuthnGuard;
use nest_rs::config::Environment;
use nest_rs::core::App;
use nest_rs::guards::{AppBuilderGuardsExt, guard};

use live::LiveModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();

    App::builder()
        .use_guards_global([guard::<AuthnGuard>()])
        .module::<LiveModule>()
        .build()
        .await?
        .run()
        .await
}

use anyhow::Result;
use features::app_authn::AppAuthnGuard;
use nest_rs::config::Environment;
use nest_rs::core::App;
use nest_rs::guards::{AppBuilderGuardsExt, guard};

use live::LiveModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();

    App::builder()
        .use_guards_global([guard::<AppAuthnGuard>()])
        .module::<LiveModule>()
        .build()
        .await?
        .run()
        .await
}

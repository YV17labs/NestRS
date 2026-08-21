use anyhow::Result;
use features::app_authn::AppAuthnGuard;
use features::app_authz::AppAuthzGuard;
use nest_rs::config::Environment;
use nest_rs::core::App;
use nest_rs::guards::{AppBuilderGuardsExt, guard};

use api::ApiModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();

    App::builder()
        .use_guards_global([guard::<AppAuthnGuard>(), guard::<AppAuthzGuard>()])
        .module::<ApiModule>()
        .build()
        .await?
        .run()
        .await
}

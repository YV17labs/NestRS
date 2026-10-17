use anyhow::Result;
use features::authn::AuthnGuard;
use nest_rs::core::App;
use nest_rs::guards::{AppBuilderGuardsExt, guard};

use live::LiveModule;

#[nest_rs::main]
async fn main() -> Result<()> {
    App::builder()
        .use_guards_global([guard::<AuthnGuard>()])
        .module::<LiveModule>()
        .build()
        .await?
        .run()
        .await
}

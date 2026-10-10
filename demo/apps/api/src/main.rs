use anyhow::Result;
use features::authn::AuthnGuard;
use features::authz::AuthzGuard;
use nest_rs::core::App;
use nest_rs::guards::{AppBuilderGuardsExt, guard};

use api::ApiModule;

#[nest_rs::main]
async fn main() -> Result<()> {
    App::builder()
        .use_guards_global([guard::<AuthnGuard>(), guard::<AuthzGuard>()])
        .module::<ApiModule>()
        .build()
        .await?
        .run()
        .await
}

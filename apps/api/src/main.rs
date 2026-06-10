use anyhow::Result;
use features::authn::AuthGuard;
use features::authz::AuthzGuard;
use nest_rs_config::Environment;
use nest_rs_core::App;
use nest_rs_guards::{AppBuilderGuardsExt, guard};
use nest_rs_opentelemetry::OpenTelemetry;

use api::ApiModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();
    let _opentelemetry = OpenTelemetry::init("api")?;

    App::builder()
        .use_guards_global([guard::<AuthGuard>(), guard::<AuthzGuard>()])
        .module::<ApiModule>()
        .build()
        .await?
        .run()
        .await
}

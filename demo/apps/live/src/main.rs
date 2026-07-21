use anyhow::Result;
use features::authn::AuthnGuard;
use nest_rs_config::Environment;
use nest_rs_core::App;
use nest_rs_guards::{AppBuilderGuardsExt, guard};
use nest_rs_opentelemetry::OpenTelemetry;

use live::LiveModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();
    let _opentelemetry = OpenTelemetry::init("live")?;

    App::builder()
        .use_guards_global([guard::<AuthnGuard>()])
        .module::<LiveModule>()
        .build()
        .await?
        .run()
        .await
}

use anyhow::Result;
use features::authn::AuthGuard;
use nest_rs_config::Environment;
use nest_rs_core::App;
use nest_rs_guards::{AppBuilderGuardsExt, guard};
use nest_rs_opentelemetry::OpenTelemetry;

use publish_live::PublishLiveModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();
    let _opentelemetry = OpenTelemetry::init("publish-live")?;

    App::builder()
        .use_guards_global([guard::<AuthGuard>()])
        .module::<PublishLiveModule>()
        .build()
        .await?
        .run()
        .await
}

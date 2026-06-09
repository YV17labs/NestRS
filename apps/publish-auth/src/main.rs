use anyhow::Result;
use nest_rs_config::Environment;
use nest_rs_core::App;
use nest_rs_opentelemetry::OpenTelemetry;

use publish_auth::PublishAuthModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();
    let _opentelemetry = OpenTelemetry::init("publish-auth")?;

    App::builder()
        .module::<PublishAuthModule>()
        .build()
        .await?
        .run()
        .await
}

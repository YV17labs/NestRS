use anyhow::Result;
use nest_rs_config::Environment;
use nest_rs_core::App;
use nest_rs_opentelemetry::OpenTelemetry;

use auth::AuthModule;

#[tokio::main]
async fn main() -> Result<()> {
    let _environment = Environment::init();
    let _opentelemetry = OpenTelemetry::init("auth")?;

    // Keyed OAuth2 clients are owned by `SocialModule` (collect-phase
    // `provide_keyed`), so `main` stays pure composition — no root-level seed.
    App::builder()
        .module::<AuthModule>()
        .build()
        .await?
        .run()
        .await
}

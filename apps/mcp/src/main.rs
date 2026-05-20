mod app;
mod weather;

use anyhow::Result;
use nestrs_core::{Container, Module};
use nestrs_health::HealthController;
use poem::{listener::TcpListener, Route, Server};

use crate::weather::WeatherController;

#[tokio::main]
async fn main() -> Result<()> {
    nestrs_core::logging::init();

    let container = app::AppModule::register(Container::builder()).build();

    let mcp = {
        let c = container.clone();
        nestrs_mcp::endpoint(move || WeatherController::from_container(&c))
    };

    let routes = Route::new()
        .nest("/mcp", mcp)
        .nest("/", HealthController::routes(&container));

    tracing::info!("mcp listening on http://0.0.0.0:3001");
    tracing::info!("  MCP streamable HTTP: http://0.0.0.0:3001/mcp");
    tracing::info!("  Liveness probe:      http://0.0.0.0:3001/health/live");

    Server::new(TcpListener::bind("0.0.0.0:3001"))
        .run(routes)
        .await?;
    Ok(())
}

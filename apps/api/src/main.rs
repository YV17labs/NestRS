mod app;
mod graphql;
mod users;

use async_graphql_poem::GraphQL;
use nestrs_core::{Container, Module};
use nestrs_health::HealthController;
use poem::{listener::TcpListener, post, Route, Server};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    nestrs_core::logging::init();

    let container = app::AppModule::register(Container::builder()).build();
    let schema = graphql::build_schema(container.clone());

    let routes = Route::new()
        .at(
            "/graphql",
            post(GraphQL::new(schema)).get(graphql::playground),
        )
        .nest("/", HealthController::routes(&container));

    tracing::info!("api listening on http://0.0.0.0:3000");
    tracing::info!("  GraphQL playground: http://0.0.0.0:3000/graphql");
    tracing::info!("  Liveness probe:     http://0.0.0.0:3000/health/live");
    tracing::info!("  Readiness probe:    http://0.0.0.0:3000/health/ready");
    tracing::info!("  Startup probe:      http://0.0.0.0:3000/health/startup");

    Server::new(TcpListener::bind("0.0.0.0:3000"))
        .run(routes)
        .await
}

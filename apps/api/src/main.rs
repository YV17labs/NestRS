mod app;
mod auth;
mod authz;
mod users;

use std::process::ExitCode;

use anyhow::{Context, Result};
use nestrs_core::{App, Container, Module};
use nestrs_http::HttpTransport;
use nestrs_telemetry::Telemetry;
use sea_orm::{Database, DatabaseConnection};

use crate::app::AppModule;

fn main() -> ExitCode {
    // The `schema` subcommand renders SDL from the resolvers without serving.
    // Building the container synchronously cannot run the async DB factory, so
    // seed a disconnected connection — the schema is described, never executed —
    // letting the DB-injected providers register.
    if std::env::args().nth(1).as_deref() == Some("schema") {
        let container = AppModule::register(
            Container::builder().provide(DatabaseConnection::default()),
        )
        .build();
        return nestrs_graphql_cli::run_with(
            &container,
            concat!(env!("CARGO_MANIFEST_DIR"), "/schema.graphql"),
            std::env::args().skip(2),
        );
    }

    match serve() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err:?}");
            ExitCode::FAILURE
        }
    }
}

#[tokio::main]
async fn serve() -> Result<()> {
    let _telemetry = Telemetry::init("api")?;
    let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?;

    App::builder()
        // The DB pool is async, so it is built at the composition root and
        // injected into providers (the same final-container contract the
        // scheduler/queue transports rely on).
        .provide_factory(move |_| async move { Ok(Database::connect(&database_url).await?) })
        .module::<AppModule>()
        .build()
        .await?
        .transport(HttpTransport::new().bind("0.0.0.0:3002"))
        .run()
        .await
}

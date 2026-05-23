mod app;
mod auth;
mod users;

use std::process::ExitCode;

use anyhow::Result;
use nestrs_core::App;
use nestrs_http::HttpTransport;
use nestrs_telemetry::Telemetry;

use crate::app::AppModule;

fn main() -> ExitCode {
    // The `schema` subcommand needs no async runtime or telemetry, so handle it
    // before booting the server.
    if std::env::args().nth(1).as_deref() == Some("schema") {
        return nestrs_graphql_cli::run::<AppModule>(
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

    App::new::<AppModule>()
        .transport(HttpTransport::new().bind("0.0.0.0:3002"))
        .run()
        .await
}

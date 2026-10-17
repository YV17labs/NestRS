//! A Postgres requiring a client certificate of one role (`src/module.rs`):
//! the `postgres-mutual-tls` dev container's, whose `pg_hba.conf` lets
//! `nestrs_certificate` in on the dev container's certificate alone. Run there,
//! by hand, when the connection's TLS changes: every other environment's
//! Postgres asks no certificate, so `test.just` leaves this module out of it.

use std::time::Duration;

use nest_rs_core::{App, module};
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmModule};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};

/// The role the server maps the dev container's certificate to.
const ROLE: &str = "nestrs_certificate";

/// What the dev container presents to a server asking for a certificate
/// (`.devcontainer/scripts/issue-certificates.sh`, its `CLIENTS`).
const CERTIFICATE: &str = "sslcert=/certs/dev/client.pem&sslkey=/certs/dev/client.key";

#[module(imports = [SeaOrmModule::for_root(None)])]
struct PoolModule;

/// The suite's URL, signed in as [`ROLE`] with no password, and `query`
/// appended.
fn as_the_role(query: Option<&str>) -> String {
    let url = crate::harness::url();
    let (scheme, rest) = url.split_once("://").expect("a URL with a scheme");
    let host = rest.split_once('@').map_or(rest, |(_, host)| host);
    let joined = format!("{scheme}://{ROLE}@{host}");
    match query {
        None => joined,
        Some(query) => {
            let joiner = if joined.contains('?') { '&' } else { '?' };
            format!("{joined}{joiner}{query}")
        }
    }
}

async fn booted(url: String) -> anyhow::Result<nest_rs_core::App> {
    App::builder()
        .provide(SeaOrmConfig {
            url,
            connect_timeout_secs: Some(60),
            ..Default::default()
        })
        .module::<PoolModule>()
        .build()
        .await
}

#[tokio::test]
async fn a_client_presenting_its_certificate_is_let_in_as_the_role_it_maps_to() {
    let app = booted(as_the_role(Some(CERTIFICATE)))
        .await
        .expect("the certificate lets the role in");
    let db = app
        .container()
        .get::<DatabaseConnection>()
        .expect("the pool is provided");
    let row = db
        .query_one_raw(Statement::from_string(
            db.get_database_backend(),
            "SELECT current_user::text AS name",
        ))
        .await
        .expect("a query runs")
        .expect("one row");
    let name: String = row.try_get("", "name").expect("the user's name");
    assert_eq!(name, ROLE);
}

#[tokio::test]
async fn a_client_presenting_none_is_refused_at_once_naming_the_variable() {
    let began = std::time::Instant::now();
    let Err(refused) = booted(as_the_role(None)).await else {
        panic!("a role let in on a certificate alone is refused without one");
    };
    let took = began.elapsed();
    assert!(took < Duration::from_secs(30), "at once, took {took:?}");
    let said = nest_rs_core::error_message(&*refused);
    assert!(
        said.contains("SQLSTATE 28000")
            && said.contains(&nest_rs_config::var_name("seaorm", "URL")),
        "{said}"
    );
}

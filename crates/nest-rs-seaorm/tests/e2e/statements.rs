//! Covers `src/statements.rs` against Postgres: once something records client
//! operations, every statement the module's pool sends is handed over by its
//! summary, under the database and the server the URL names — every one sea-orm
//! times, which leaves out `execute_unprepared` and `stream`.

use std::sync::Mutex;

use nest_rs_core::client_operation::{ClientOperation, ClientValue};
use nest_rs_core::{App, module};
use nest_rs_seaorm::SeaOrmModule;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

/// Each operation the seeded recorder was handed: its name, and the database
/// it named.
static RECORDED: Mutex<Vec<(String, Option<String>)>> = Mutex::new(Vec::new());

fn capture(operation: &ClientOperation<'_>) {
    let namespace = operation
        .attributes()
        .iter()
        .find_map(|(key, value)| match (*key, value) {
            ("db.namespace", ClientValue::Text(namespace)) => Some((*namespace).to_owned()),
            _ => None,
        });
    RECORDED
        .lock()
        .expect("the recorded operations")
        .push((operation.name().to_owned(), namespace));
}

#[module(imports = [SeaOrmModule])]
struct PoolModule;

#[tokio::test]
async fn every_statement_the_pool_sends_is_recorded_by_its_summary() {
    nest_rs_core::__private::set_client_recorder(capture);
    let app = App::builder()
        .provide(crate::module::config(None))
        .module::<PoolModule>()
        .build()
        .await
        .expect("boots");
    let db = app
        .container()
        .get::<DatabaseConnection>()
        .expect("the pool");

    let statement = |sql: &str| Statement::from_string(DbBackend::Postgres, sql);
    db.query_one_raw(statement("SELECT 1")).await.expect("runs");
    db.execute_raw(statement("select current_database()"))
        .await
        .expect("runs");

    let database = crate::harness::url()
        .parse::<sea_orm::sqlx::postgres::PgConnectOptions>()
        .expect("a Postgres URL")
        .get_database()
        .map(str::to_owned);
    let recorded = RECORDED.lock().expect("the recorded operations").clone();
    assert_eq!(
        recorded,
        vec![
            ("SELECT".to_owned(), database.clone()),
            ("SELECT".to_owned(), database),
        ],
    );
}

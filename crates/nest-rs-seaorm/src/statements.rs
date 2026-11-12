//! Every statement the app's pool sends, handed to the observability stack as
//! the client operation the database conventions name — its summary, its
//! database and its server, never its text or a value it binds — once the
//! database answered it.
//!
//! sea-orm times each statement and calls back with it; nothing installs the
//! callback where nothing records client operations.

use std::time::SystemTime;

use nest_rs_core::client_operation::{self, ClientDuration, ClientOperation, ClientValue};
use nest_rs_core::operation_log::OTHER;
use sea_orm::DatabaseConnection;
use sea_orm::metric::Info;
use sea_orm::sqlx::postgres::PgConnectOptions;

use crate::query_summary::QuerySummary;

/// The database conventions' client duration, keyed by what a statement does,
/// to which table, in which database on which server.
const STATEMENT: ClientDuration = ClientDuration::new(
    "db.client.operation.duration",
    "Duration of database client operations.",
    &[
        DB_SYSTEM_NAME,
        DB_NAMESPACE,
        DB_OPERATION_NAME,
        DB_COLLECTION_NAME,
        DB_QUERY_SUMMARY,
        SERVER_ADDRESS,
        SERVER_PORT,
    ],
);

const DB_SYSTEM_NAME: &str = "db.system.name";
const DB_NAMESPACE: &str = "db.namespace";
const DB_OPERATION_NAME: &str = "db.operation.name";
const DB_COLLECTION_NAME: &str = "db.collection.name";
const DB_QUERY_SUMMARY: &str = "db.query.summary";
const SERVER_ADDRESS: &str = "server.address";
const SERVER_PORT: &str = "server.port";

/// The conventions' name for the database this driver speaks to.
const POSTGRESQL: &str = "postgresql";

/// What every statement of one pool is recorded under: its database and its
/// server, as the URL names them.
pub(crate) struct Statements {
    namespace: Option<String>,
    address: String,
    port: i64,
}

impl Statements {
    /// Time every statement `db`, opened with `options`, sends — when anything
    /// records client operations.
    pub(crate) fn time(db: &mut DatabaseConnection, options: &PgConnectOptions) {
        if !client_operation::is_recorded() {
            return;
        }
        let statements = Self::of(options);
        db.set_metric_callback(move |info| statements.record(info));
    }

    fn of(options: &PgConnectOptions) -> Self {
        Self {
            namespace: options.get_database().map(str::to_owned),
            address: options.get_host().to_owned(),
            port: i64::from(options.get_port()),
        }
    }

    /// Hand the statement `info` reports to the recorder.
    fn record(&self, info: &Info<'_>) {
        let summary = QuerySummary::of(&info.statement.sql);
        let text = summary.as_ref().map(QuerySummary::text);
        let mut attributes = Vec::with_capacity(STATEMENT.keys().len());
        attributes.push((DB_SYSTEM_NAME, ClientValue::Text(POSTGRESQL)));
        if let Some(namespace) = &self.namespace {
            attributes.push((DB_NAMESPACE, ClientValue::Text(namespace)));
        }
        if let Some(summary) = &summary {
            attributes.push((DB_OPERATION_NAME, ClientValue::Text(summary.operation())));
            if let Some(collection) = summary.collection() {
                attributes.push((DB_COLLECTION_NAME, ClientValue::Text(collection)));
            }
        }
        if let Some(text) = &text {
            attributes.push((DB_QUERY_SUMMARY, ClientValue::Text(text)));
        }
        attributes.push((SERVER_ADDRESS, ClientValue::Text(&self.address)));
        attributes.push((SERVER_PORT, ClientValue::Integer(self.port)));

        let answered = SystemTime::now();
        let operation = ClientOperation::new(
            &STATEMENT,
            text.as_deref().unwrap_or(POSTGRESQL),
            answered.checked_sub(info.elapsed).unwrap_or(answered),
            info.elapsed,
            &attributes,
        );
        // sea-orm reports a failure without its error; the conventions' class
        // for one with no code readily available is `_OTHER`.
        if info.failed {
            operation.failed(OTHER).record();
        } else {
            operation.record();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pool_is_recorded_under_the_database_and_the_server_its_url_names() {
        let options: PgConnectOptions = "postgres://nestrs:secret@db.internal:6543/publish"
            .parse()
            .expect("a Postgres URL");
        let statements = Statements::of(&options);
        assert_eq!(statements.namespace.as_deref(), Some("publish"));
        assert_eq!(statements.address, "db.internal");
        assert_eq!(statements.port, 6543);
    }

    /// What the recorder this suite seeds was handed: each operation's name,
    /// its attributes as written, and its class if it failed.
    type Recorded = (String, Vec<(String, String)>, Option<String>);

    static RECORDED: std::sync::Mutex<Vec<Recorded>> = std::sync::Mutex::new(Vec::new());

    fn capture(operation: &ClientOperation<'_>) {
        let attributes = operation
            .attributes()
            .iter()
            .map(|(key, value)| {
                let value = match value {
                    ClientValue::Text(text) => (*text).to_owned(),
                    ClientValue::Integer(integer) => integer.to_string(),
                    _ => unreachable!("the driver writes text and integers"),
                };
                ((*key).to_owned(), value)
            })
            .collect();
        RECORDED.lock().expect("the recorded operations").push((
            operation.name().to_owned(),
            attributes,
            operation.error_type().map(str::to_owned),
        ));
    }

    #[test]
    fn a_statement_is_recorded_by_its_summary_and_never_by_what_it_binds() {
        nest_rs_core::__private::set_client_recorder(capture);
        let options: PgConnectOptions = "postgres://nestrs:secret@db.internal:6543/publish"
            .parse()
            .expect("a Postgres URL");
        let statements = Statements::of(&options);
        let statement = sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            r#"SELECT "posts"."id" FROM "posts" WHERE "posts"."title" = $1"#,
            [sea_orm::Value::from("a title only the caller knows")],
        );

        for failed in [false, true] {
            statements.record(&Info {
                elapsed: std::time::Duration::from_millis(2),
                statement: &statement,
                failed,
            });
        }

        let recorded = RECORDED.lock().expect("the recorded operations").clone();
        let pairs = |set: &[(&str, &str)]| -> Vec<(String, String)> {
            set.iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect()
        };
        let attributes = pairs(&[
            (DB_SYSTEM_NAME, POSTGRESQL),
            (DB_NAMESPACE, "publish"),
            (DB_OPERATION_NAME, "SELECT"),
            (DB_COLLECTION_NAME, "posts"),
            (DB_QUERY_SUMMARY, "SELECT posts"),
            (SERVER_ADDRESS, "db.internal"),
            (SERVER_PORT, "6543"),
        ]);
        assert_eq!(
            recorded,
            vec![
                ("SELECT posts".to_owned(), attributes.clone(), None),
                (
                    "SELECT posts".to_owned(),
                    attributes,
                    Some(OTHER.to_owned())
                ),
            ],
        );
        assert!(
            !format!("{recorded:?}").contains("only the caller knows"),
            "{recorded:?}"
        );
    }
}

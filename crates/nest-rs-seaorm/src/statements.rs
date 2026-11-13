//! Every statement the app's pool sends, handed to the observability stack as
//! the client operation the database conventions name — its summary, its
//! database and its server, never its text or a value it binds — once the
//! database answered it.
//!
//! sea-orm times each statement it prepares and calls back with it; what it
//! sends untimed — `BEGIN`, `COMMIT`, `ROLLBACK` and `execute_unprepared`'s text
//! — the driver times itself through [`Statements::sent`]. Nothing is installed,
//! and no clock read, where nothing records client operations.

use std::future::Future;
use std::sync::Arc;
use std::time::SystemTime;

use nest_rs_core::client_operation::{
    self, ClientOperation, ClientValue, DB_CLIENT_OPERATION_DURATION, DB_COLLECTION_NAME,
    DB_NAMESPACE, DB_OPERATION_NAME, DB_QUERY_SUMMARY, DB_RESPONSE_STATUS_CODE, DB_SYSTEM_NAME,
    PendingClientOperation, SERVER_ADDRESS, SERVER_PORT,
};
use nest_rs_core::operation_log::OTHER;
use sea_orm::metric::Info;
use sea_orm::sqlx::postgres::PgConnectOptions;
use sea_orm::{DatabaseConnection, DbErr};

use crate::pool::SeaOrmPool;
use crate::query_summary::QuerySummary;

/// The conventions' name for the database this driver speaks to.
const POSTGRESQL: &str = "postgresql";

/// What every statement of one pool is recorded under: its database and its
/// server, as the options its connections open with name them.
pub(crate) struct Statements {
    options: Arc<PgConnectOptions>,
}

/// What the callback sea-orm calls for each statement holds: the recorder, and
/// the pool's connections, observed for as long as the pool's handle lives.
struct Recorder {
    statements: Statements,
    _observed: Option<Arc<SeaOrmPool>>,
}

impl Recorder {
    /// Through `self` whole, so the callback holds the observation too: a
    /// closure naming one field captures that field alone.
    fn record(&self, info: &Info<'_>) {
        self.statements.record(info);
    }
}

impl Statements {
    /// Time every statement `db` sends, and observe its connections, for as
    /// long as `db` lives — where anything records client operations.
    pub(crate) fn install(db: &mut DatabaseConnection) {
        let Some(statements) = Self::recording(db) else {
            return;
        };
        let recorder = Recorder {
            statements,
            _observed: SeaOrmPool::observed(db),
        };
        db.set_metric_callback(move |info| recorder.record(info));
    }

    /// What `pool`'s statements are recorded under, where anything records
    /// client operations and `pool` is a Postgres pool.
    pub(crate) fn recording(pool: &DatabaseConnection) -> Option<Self> {
        if !client_operation::is_recorded() {
            return None;
        }
        match pool.inner {
            sea_orm::DatabaseConnectionType::SqlxPostgresPoolConnection(_) => Some(Self::on(
                pool.get_postgres_connection_pool().connect_options(),
            )),
            _ => None,
        }
    }

    const fn on(options: Arc<PgConnectOptions>) -> Self {
        Self { options }
    }

    /// Await `statement`, the text `sql` sea-orm sends without timing it, and
    /// hand it over once the database answered — or once its caller gave up on
    /// it, a unit cut at its deadline — where `statements` records it.
    pub(crate) async fn sent<T>(
        statements: Option<&Self>,
        sql: &str,
        statement: impl Future<Output = Result<T, DbErr>>,
    ) -> Result<T, DbErr> {
        let Some(statements) = statements else {
            return statement.await;
        };
        let pending = statements.described(sql, |name, attributes| {
            PendingClientOperation::start(
                &DB_CLIENT_OPERATION_DURATION,
                name.to_owned(),
                attributes,
            )
        });
        let answer = statement.await;
        match answer.as_ref().err().map(Refusal::of) {
            None => pending.ended(&[], None),
            Some(Refusal::Answered(code)) => pending.ended(
                &[(DB_RESPONSE_STATUS_CODE, ClientValue::Text(&code))],
                Some(&code),
            ),
            Some(Refusal::Unanswered(class)) => pending.ended(&[], Some(class)),
        }
        answer
    }

    /// Hand the statement sea-orm timed and reports in `info` to the recorder;
    /// a failure, which sea-orm reports without its error, is the conventions'
    /// `_OTHER`.
    fn record(&self, info: &Info<'_>) {
        let answered = SystemTime::now();
        let started = answered.checked_sub(info.elapsed).unwrap_or(answered);
        self.described(&info.statement.sql, |name, attributes| {
            ClientOperation::new(
                &DB_CLIENT_OPERATION_DURATION,
                name,
                started,
                info.elapsed,
                attributes,
            )
            .failed(info.failed.then_some(OTHER))
            .record();
        });
    }

    /// What `sql` is recorded as — its name and its attributes — handed to
    /// `with`.
    fn described<R>(
        &self,
        sql: &str,
        with: impl FnOnce(&str, &[(&'static str, ClientValue<'_>)]) -> R,
    ) -> R {
        let summary = QuerySummary::of(sql);
        let text = summary.as_ref().map(QuerySummary::text);
        let mut attributes = Vec::with_capacity(DB_CLIENT_OPERATION_DURATION.attributes().len());
        attributes.push((DB_SYSTEM_NAME, ClientValue::Text(POSTGRESQL)));
        if let Some(namespace) = self.options.get_database() {
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
        attributes.push((SERVER_ADDRESS, ClientValue::Text(self.options.get_host())));
        attributes.push((
            SERVER_PORT,
            ClientValue::Integer(i64::from(self.options.get_port())),
        ));

        with(text.as_deref().unwrap_or(POSTGRESQL), &attributes)
    }
}

/// How a statement the driver timed failed.
enum Refusal {
    /// The server refused it with this SQLSTATE.
    Answered(String),
    /// No server answered: `timeout` for a connection the pool never handed
    /// over in time — the statement never left — `_OTHER` otherwise.
    Unanswered(&'static str),
}

impl Refusal {
    fn of(err: &DbErr) -> Self {
        if let Some(code) = crate::error::server_error(err).and_then(|server| server.code()) {
            return Self::Answered(code.into_owned());
        }
        match err {
            DbErr::ConnectionAcquire(sea_orm::ConnAcquireErr::Timeout) => Self::Unanswered(TIMEOUT),
            _ => Self::Unanswered(OTHER),
        }
    }
}

/// The class of an operation that never answered because its wait ran out,
/// as the HTTP client's.
const TIMEOUT: &str = "timeout";

/// Await `statement`, the text `sql` sea-orm sends on `pool` without timing it,
/// and hand it over once the database answered.
pub(crate) async fn sent<T>(
    pool: &DatabaseConnection,
    sql: &str,
    statement: impl Future<Output = Result<T, DbErr>>,
) -> Result<T, DbErr> {
    Statements::sent(Statements::recording(pool).as_ref(), sql, statement).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pool_is_recorded_under_the_database_and_the_server_its_url_names() {
        let options: PgConnectOptions = "postgres://nestrs:secret@db.internal:6543/publish"
            .parse()
            .expect("a Postgres URL");
        let statements = Statements::on(Arc::new(options));
        assert_eq!(statements.options.get_database(), Some("publish"));
        assert_eq!(statements.options.get_host(), "db.internal");
        assert_eq!(statements.options.get_port(), 6543);
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
        let statements = Statements::on(Arc::new(options));
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

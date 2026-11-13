//! Every statement the app's pool sends, handed to the observability stack as
//! the client operation the database conventions name — its summary, its
//! database and its server, never its text or a value it binds — once the
//! database answered it.
//!
//! sea-orm times each statement it prepares and calls back with it; what it
//! sends untimed — `BEGIN`, `COMMIT`, `ROLLBACK` and `execute_unprepared`'s text
//! — the driver times itself through [`Statements::sent`]. Nothing is installed,
//! and no clock read, where nothing records client operations; nothing is
//! described where nothing would record the statement sent.

use std::borrow::Cow;
use std::future::Future;
use std::sync::Arc;
use std::time::SystemTime;

use nest_rs_core::client_operation::{
    self, ClientOperation, ClientValue, DB_CLIENT_OPERATION_DURATION, DB_COLLECTION_NAME,
    DB_NAMESPACE, DB_OPERATION_NAME, DB_QUERY_SUMMARY, DB_SYSTEM_NAME, DatabaseFailure,
    PendingClientOperation, SERVER_ADDRESS, SERVER_PORT,
};
use nest_rs_core::operation_log::OTHER;
use sea_orm::metric::Info;
use sea_orm::sqlx::postgres::PgConnectOptions;
use sea_orm::{DatabaseConnection, DbErr};

use crate::pool::{SeaOrmPool, is_acquire_timeout};
use crate::query_summary::QuerySummary;

/// The conventions' name for the database this driver speaks to.
const POSTGRESQL: &str = "postgresql";

/// What every statement of one pool is recorded under — its database and its
/// server, as the options its connections open with name them — or nothing,
/// where nothing records client operations.
#[derive(Clone, Default)]
pub(crate) struct Statements {
    options: Option<Arc<PgConnectOptions>>,
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
        let statements = Self::of(db);
        if statements.options.is_none() {
            return;
        }
        let recorder = Recorder {
            statements,
            _observed: SeaOrmPool::observed(db),
        };
        db.set_metric_callback(move |info| recorder.record(info));
    }

    /// What `pool`'s statements are recorded under: nothing where nothing
    /// records client operations, or where `pool` holds no Postgres pool.
    pub(crate) fn of(pool: &DatabaseConnection) -> Self {
        let options = (client_operation::is_recorded()
            && matches!(
                pool.inner,
                sea_orm::DatabaseConnectionType::SqlxPostgresPoolConnection(_)
            ))
        .then(|| pool.get_postgres_connection_pool().connect_options());
        Self { options }
    }

    /// Await `statement`, the text `sql` sea-orm sends without timing it, and
    /// hand it over once the database answered — or once its caller gave up on
    /// it, a unit cut at its deadline — where it is recorded.
    pub(crate) async fn sent<T>(
        &self,
        sql: &str,
        statement: impl Future<Output = Result<T, DbErr>>,
    ) -> Result<T, DbErr> {
        let Some(options) = self
            .options
            .as_deref()
            .filter(|_| client_operation::is_recorded_here())
        else {
            return statement.await;
        };
        let pending = described(options, sql, |name, attributes| {
            PendingClientOperation::start(&DB_CLIENT_OPERATION_DURATION, name.clone(), attributes)
        });
        let answer = statement.await;
        match &answer {
            Ok(_) => pending.ended_with(Ok(())),
            Err(err) => {
                let code = crate::error::server_error(err).and_then(|server| server.code());
                pending.ended_with(Err(match code.as_deref() {
                    Some(code) => DatabaseFailure::Refused(code),
                    None if is_acquire_timeout(err) => DatabaseFailure::TimedOut,
                    None => DatabaseFailure::Unanswered,
                }));
            }
        }
        answer
    }

    /// Hand the statement sea-orm timed and reports in `info` to the recorder;
    /// a failure, which sea-orm reports without its error, is the conventions'
    /// `_OTHER`.
    fn record(&self, info: &Info<'_>) {
        let Some(options) = &self.options else {
            return;
        };
        let answered = SystemTime::now();
        let started = answered.checked_sub(info.elapsed).unwrap_or(answered);
        described(options, &info.statement.sql, |name, attributes| {
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
}

/// What `sql`, sent to the pool `options` open, is recorded as — its name, which
/// is its summary, and its attributes — handed to `with`. A summary naming no
/// table is its operation's word, which allocates nothing: a transaction's
/// `BEGIN`, `COMMIT` and `ROLLBACK`.
fn described<R>(
    options: &PgConnectOptions,
    sql: &str,
    with: impl FnOnce(&Cow<'static, str>, &[(&'static str, ClientValue<'_>)]) -> R,
) -> R {
    let summary = QuerySummary::of(sql);
    let name: Cow<'static, str> = match &summary {
        Some(summary) if summary.collection().is_none() => Cow::Borrowed(summary.operation()),
        Some(summary) => Cow::Owned(summary.text()),
        None => Cow::Borrowed(POSTGRESQL),
    };
    let mut attributes = Vec::with_capacity(DB_CLIENT_OPERATION_DURATION.attributes().len());
    attributes.push((DB_SYSTEM_NAME, ClientValue::Text(POSTGRESQL)));
    if let Some(namespace) = options.get_database() {
        attributes.push((DB_NAMESPACE, ClientValue::Text(namespace)));
    }
    if let Some(summary) = &summary {
        attributes.push((DB_OPERATION_NAME, ClientValue::Text(summary.operation())));
        if let Some(collection) = summary.collection() {
            attributes.push((DB_COLLECTION_NAME, ClientValue::Text(collection)));
        }
        attributes.push((DB_QUERY_SUMMARY, ClientValue::Text(&name)));
    }
    attributes.push((SERVER_ADDRESS, ClientValue::Text(options.get_host())));
    attributes.push((
        SERVER_PORT,
        ClientValue::Integer(i64::from(options.get_port())),
    ));
    with(&name, &attributes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pool_is_recorded_under_the_database_and_the_server_its_url_names() {
        let options: PgConnectOptions = "postgres://nestrs:secret@db.internal:6543/publish"
            .parse()
            .expect("a Postgres URL");
        let statements = Statements {
            options: Some(Arc::new(options)),
        };
        let options = statements.options.as_deref().expect("recorded");
        assert_eq!(options.get_database(), Some("publish"));
        assert_eq!(options.get_host(), "db.internal");
        assert_eq!(options.get_port(), 6543);
    }

    /// What the recorder this suite seeds was handed: each operation's name,
    /// its attributes as written, and its class if it failed.
    type Recorded = (String, Vec<(String, String)>, Option<String>);

    static RECORDED: std::sync::Mutex<Vec<Recorded>> = std::sync::Mutex::new(Vec::new());

    fn capture(operation: &ClientOperation<'_>) {
        let attributes = operation
            .attributes()
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.to_string()))
            .collect();
        RECORDED.lock().expect("the recorded operations").push((
            operation.name().to_owned(),
            attributes,
            operation.error_type().map(str::to_owned),
        ));
    }

    #[test]
    fn a_statement_is_recorded_by_its_summary_and_never_by_what_it_binds() {
        nest_rs_core::__private::set_client_recorder(capture, true);
        let options: PgConnectOptions = "postgres://nestrs:secret@db.internal:6543/publish"
            .parse()
            .expect("a Postgres URL");
        let statements = Statements {
            options: Some(Arc::new(options)),
        };
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

//! `ServiceError` stays unprefixed: it is written in every app service's
//! signature, like `Service` itself.
//!
//! A database failure is classified once, here ([`DbOutcome`]), and said once:
//! this crate's [`SafeLink`](nest_rs_core::SafeLink) says a sea-orm or sqlx
//! error by its SQLSTATE and the names the server reported, never by the
//! server's message, which quotes the input it refused.

use std::borrow::Cow;
use std::error::Error;
use std::sync::Arc;

use nest_rs_core::__private::OpaqueCause;
use nest_rs_core::DecodeError;
use nest_rs_database::AtomicError;
use sea_orm::sqlx::error::DatabaseError;
use sea_orm::sqlx::postgres::PgDatabaseError;
use sea_orm::{DbErr, RuntimeErr, SqlxError};
use validator::ValidationErrors;

/// Failure modes shared by every service method. The business variants carry a
/// client-facing message and map to their 4xx; the opaque ones (`Db`,
/// `Internal`, `Masking`) answer a constant on every edge and keep their cause,
/// said through [`error_message`](fn@nest_rs_core::error_message), for the
/// operator's line.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum ServiceError {
    /// Edge validation (`validator`) rejected the input — maps to a 422. The
    /// fields ride under `errors` ([`field_errors`](Self::field_errors)), never
    /// `validator`'s `Debug`, which echoes the rejected value.
    #[error("validation failed")]
    Validation(#[from] ValidationErrors),
    /// A `Repo`/ORM query failed for a reason the caller cannot act on
    /// ([`DbOutcome::Internal`], a `500`), or Postgres cancelled it at its
    /// bound ([`DbOutcome::Unavailable`], a `503`). The `DbErr` stays for
    /// `tracing`, said by its SQLSTATE; the wire sees a generic message.
    #[error("database error")]
    Db(#[source] DbErr),
    /// Response masking could not reconcile a loaded row into its wire DTO: a
    /// 500. The decode failure is kept as its [`DecodeError`] report, never
    /// serde's sentence, which quotes the row's values. Built by the
    /// relation loaders `#[expose]` emits.
    #[error("response masking failed")]
    #[non_exhaustive]
    Masking {
        /// What did not decode, without the value.
        source: DecodeError,
    },
    /// A well-formed request the service rejects on business grounds (empty
    /// body, non-positive amount). Maps to **422**; the message is client-facing.
    #[error("{0}")]
    Invalid(String),
    /// The action conflicts with the resource's current state (spending past a
    /// balance, acting on a closed record). Maps to **409**; client-facing.
    #[error("{0}")]
    Conflict(String),
    /// The caller is known but not permitted to perform this action. Maps to
    /// **403**; client-facing.
    #[error("{0}")]
    Forbidden(String),
    /// The addressed resource does not exist (or is deliberately hidden from
    /// this caller). Maps to **404**; client-facing.
    #[error("{0}")]
    NotFound(String),
    /// An internal failure that is not a `DbErr` (a hash, an enqueue push, an
    /// upstream call). Maps to **500**: it displays the constant every edge
    /// answers, and its [`source`](Error::source) says the detail, then the
    /// cause's chain, for the operator's line. Built by
    /// [`internal`](Self::internal) or, with what failed,
    /// [`internal_because`](Self::internal_because).
    #[error("{}", nest_rs_core::OPAQUE_CLIENT_MESSAGE)]
    #[non_exhaustive]
    Internal {
        /// The detail and its cause, shared so the error stays `Clone` for
        /// dataloaders; read through [`Error::source`].
        #[doc(hidden)]
        #[source]
        cause: OpaqueCause,
    },
}

impl ServiceError {
    /// **422** — a well-formed request the service rejects on business grounds.
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }

    /// **409** — the action conflicts with the resource's current state.
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }

    /// **403** — the caller is authenticated but not permitted.
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::Forbidden(msg.into())
    }

    /// **404** — the addressed resource does not exist (or is hidden).
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    /// **500** — an internal failure that is not a database error. The detail
    /// reaches the operator's line only; the wire sees a constant string. It
    /// names what failed, never a value the request carried.
    pub fn internal(detail: impl Into<Cow<'static, str>>) -> Self {
        Self::Internal {
            cause: OpaqueCause::new(detail, None),
        }
    }

    /// [`internal`](Self::internal), keeping what failed beneath the detail:
    /// the operator's line says the detail, then the cause's chain, and no
    /// edge answers anything the cause carries. A source never quotes a value
    /// the request carried — a decode failure or a driver's error is said
    /// without it, any other error as it displays.
    ///
    /// ```
    /// use nest_rs_seaorm::ServiceError;
    ///
    /// let refused = std::io::Error::other("the key store refused the connection");
    /// let err = ServiceError::internal_because("could not sign the export", refused);
    /// assert_eq!(err.to_string(), nest_rs_core::OPAQUE_CLIENT_MESSAGE);
    /// assert_eq!(
    ///     nest_rs_core::error_message(&err),
    ///     "internal error: could not sign the export: the key store refused the connection",
    /// );
    /// ```
    pub fn internal_because(
        detail: impl Into<Cow<'static, str>>,
        source: impl Into<Box<dyn Error + Send + Sync>> + 'static,
    ) -> Self {
        Self::Internal {
            cause: OpaqueCause::new(detail, Some(Arc::from(nest_rs_core::boxed_error(source)))),
        }
    }

    /// The field-level errors a [`Validation`](Self::Validation) failure carries,
    /// as the JSON every transport ships under `errors`, without the rules'
    /// parameters, which hold the rejected input.
    pub fn field_errors(&self) -> Option<serde_json::Value> {
        match self {
            Self::Validation(errors) => Some(nest_rs_pipes::validation_details(errors)),
            _ => None,
        }
    }

    /// Whether Postgres cancelled the statement at its bound: a `503`, said at
    /// `warn`, where any other `Db` is a `500`.
    fn is_unavailable(&self) -> bool {
        matches!(self, Self::Db(err) if DbOutcome::of(err) == DbOutcome::Unavailable)
    }
}

/// A response mask's decode failure, said without the row's values: what the
/// relation loaders `#[expose]` emits answer. Reached through the crate's
/// `__private`.
pub fn masking(failure: &serde_json::Error) -> ServiceError {
    ServiceError::Masking {
        source: DecodeError::new(failure),
    }
}

/// A `DbErr` classified by [`DbOutcome`]: a unique violation is a
/// [`Conflict`](ServiceError::Conflict) (`409`), a row a write found gone is
/// [`NotFound`](ServiceError::NotFound) (`404`), anything else is
/// [`Db`](ServiceError::Db) — a `503` for a statement Postgres cancelled, a
/// `500` otherwise.
impl From<DbErr> for ServiceError {
    fn from(err: DbErr) -> Self {
        match DbOutcome::of(&err) {
            DbOutcome::Conflict => {
                Self::Conflict("the request conflicts with an existing record".into())
            }
            DbOutcome::NotFound => Self::NotFound("not found".into()),
            DbOutcome::Unavailable | DbOutcome::Internal => Self::Db(err),
        }
    }
}

/// An atomic block that could not keep its promise: a database refusal by its
/// [`DbOutcome`], anything else an opaque [`Internal`](ServiceError::Internal)
/// whose cause is the `AtomicError`.
impl From<AtomicError> for ServiceError {
    fn from(err: AtomicError) -> Self {
        let err = match err {
            AtomicError::Database(source) => match source.downcast::<DbErr>() {
                Ok(db) => return Self::from(*db),
                Err(other) => AtomicError::Database(other),
            },
            other => other,
        };
        Self::internal_because("the atomic block failed", err)
    }
}

/// A failure inside an atomic block whose body speaks the driver's own
/// [`DbErr`], folded back into one: `DbErr` cannot take an `AtomicError`
/// itself.
pub(crate) enum AtomicDbError {
    Db(DbErr),
    Atomic(AtomicError),
}

impl From<DbErr> for AtomicDbError {
    fn from(err: DbErr) -> Self {
        Self::Db(err)
    }
}

impl From<AtomicError> for AtomicDbError {
    fn from(err: AtomicError) -> Self {
        Self::Atomic(err)
    }
}

impl From<AtomicDbError> for DbErr {
    fn from(err: AtomicDbError) -> Self {
        match err {
            AtomicDbError::Db(err) => err,
            AtomicDbError::Atomic(AtomicError::Database(source)) => {
                match source.downcast::<DbErr>() {
                    Ok(db) => *db,
                    Err(other) => DbErr::Custom(nest_rs_core::error_message(&*other)),
                }
            }
            AtomicDbError::Atomic(other) => DbErr::Custom(other.to_string()),
        }
    }
}

/// What a database failure means to a caller, read off the error alone — the
/// one classification `ServiceError`, `#[crud]` and every edge share.
///
/// ```
/// use nest_rs_seaorm::DbOutcome;
/// use nest_rs_seaorm::sea_orm::DbErr;
///
/// assert_eq!(DbOutcome::of(&DbErr::RecordNotUpdated), DbOutcome::NotFound);
/// assert_eq!(DbOutcome::of(&DbErr::RecordNotInserted), DbOutcome::Internal);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DbOutcome {
    /// A unique constraint refused the write (SQLSTATE `23505`): `409`,
    /// `CONFLICT`.
    Conflict,
    /// The row an update or delete addressed is no longer there: `404`,
    /// `NOT_FOUND`.
    NotFound,
    /// Postgres cancelled the statement (SQLSTATE `57014`), past its
    /// `statement_timeout` most often: the server could not serve it in time,
    /// `503`, `UNAVAILABLE`, said at `warn`. Inside a transaction nothing it
    /// wrote stands, so another attempt may succeed.
    Unavailable,
    /// Anything else, answered opaquely and logged. `RecordNotInserted` is one:
    /// sea-orm also raises it for an `ON CONFLICT DO NOTHING` that inserted
    /// nothing, so only `#[crud]`, whose create can raise it for its scope
    /// re-check alone, reads it as a refusal.
    Internal,
}

impl DbOutcome {
    /// The class of `err`.
    pub fn of(err: &DbErr) -> Self {
        match err {
            DbErr::RecordNotUpdated | DbErr::RecordNotFound(_) => Self::NotFound,
            _ => match server_error(err) {
                Some(server) if server.is_unique_violation() => Self::Conflict,
                Some(server) if server.code().as_deref() == Some(CANCELLED) => Self::Unavailable,
                _ => Self::Internal,
            },
        }
    }
}

/// SQLSTATE `query_canceled`: a statement Postgres cancelled, at its
/// `statement_timeout` or by an operator.
pub(crate) const CANCELLED: &str = "57014";

/// The error the database server answered, when `err` carries one.
fn server_error(err: &DbErr) -> Option<&(dyn DatabaseError + 'static)> {
    match err {
        DbErr::Conn(RuntimeErr::SqlxError(sqlx))
        | DbErr::Exec(RuntimeErr::SqlxError(sqlx))
        | DbErr::Query(RuntimeErr::SqlxError(sqlx)) => sqlx.as_database_error(),
        _ => None,
    }
}

nest_rs_core::inventory::submit! { nest_rs_core::SafeLink::new(say) }

/// What a sea-orm or sqlx error means, said without a value: a server error by
/// its SQLSTATE and the names it reported; a variant whose text can hold a value
/// by its kind alone; every other by its own sentence, which holds none.
fn say(link: &(dyn std::error::Error + 'static)) -> Option<String> {
    if let Some(err) = link.downcast_ref::<DbErr>() {
        return Some(said_db(err));
    }
    if let Some(err) = link.downcast_ref::<RuntimeErr>() {
        return Some(said_runtime(err));
    }
    link.downcast_ref::<SqlxError>().map(said_sqlx)
}

fn said_db(err: &DbErr) -> String {
    match err {
        DbErr::Conn(runtime) => format!("Connection Error: {}", said_runtime(runtime)),
        DbErr::Exec(runtime) => format!("Execution Error: {}", said_runtime(runtime)),
        DbErr::Query(runtime) => format!("Query Error: {}", said_runtime(runtime)),
        DbErr::TryIntoErr { from, into, .. } => format!("Error converting `{from}` into `{into}`"),
        DbErr::Type(_) => "Type Error".to_owned(),
        DbErr::Json(_) => "Json Error".to_owned(),
        DbErr::Migration(_) => "Migration Error".to_owned(),
        DbErr::RbacError(_) => "RBAC error".to_owned(),
        // Each names a table, a column, a pool's state, a count or a sentence
        // the framework wrote: never a value a client sent.
        DbErr::ConnectionAcquire(_)
        | DbErr::ConvertFromU64(_)
        | DbErr::UnpackInsertId
        | DbErr::UpdateGetPrimaryKey
        | DbErr::RecordNotFound(_)
        | DbErr::AttrNotSet(_)
        | DbErr::Custom(_)
        | DbErr::RecordNotInserted
        | DbErr::RecordNotUpdated
        | DbErr::BackendNotSupported { .. }
        | DbErr::KeyArityMismatch { .. }
        | DbErr::PrimaryKeyNotSet { .. }
        | DbErr::AccessDenied { .. }
        | DbErr::MutexPoisonError => err.to_string(),
        _ => "an unnamed sea-orm error".to_owned(),
    }
}

fn said_runtime(err: &RuntimeErr) -> String {
    match err {
        RuntimeErr::SqlxError(sqlx) => said_sqlx(sqlx),
        RuntimeErr::Internal(_) => "an internal error in sea-orm".to_owned(),
    }
}

fn said_sqlx(err: &SqlxError) -> String {
    match err {
        SqlxError::Database(server) => said_server(&**server),
        SqlxError::Io(io) => format!("error communicating with database: {}", io.kind()),
        SqlxError::Tls(tls) => format!(
            "error occurred while attempting to establish a TLS connection: {}",
            nest_rs_core::error_message(&**tls),
        ),
        SqlxError::Configuration(_) => "error with configuration".to_owned(),
        SqlxError::InvalidArgument(_) => "invalid argument".to_owned(),
        SqlxError::Protocol(_) => "encountered unexpected or invalid data".to_owned(),
        SqlxError::ColumnDecode { index, .. } => {
            format!("error occurred while decoding column {index}")
        }
        SqlxError::Encode(_) => "error occurred while encoding a value".to_owned(),
        SqlxError::Decode(_) => "error occurred while decoding".to_owned(),
        SqlxError::AnyDriverError(_) => "error in Any driver mapping".to_owned(),
        SqlxError::RowNotFound
        | SqlxError::TypeNotFound { .. }
        | SqlxError::ColumnIndexOutOfBounds { .. }
        | SqlxError::ColumnNotFound(_)
        | SqlxError::PoolTimedOut
        | SqlxError::PoolClosed
        | SqlxError::WorkerCrashed
        | SqlxError::InvalidSavePointStatement
        | SqlxError::BeginFailed => err.to_string(),
        _ => "an unnamed sqlx error".to_owned(),
    }
}

/// `database error <SQLSTATE>`, then the constraint, table and column the
/// server named: schema an operator searches for, where its message quotes the
/// row.
fn said_server(server: &(dyn DatabaseError + 'static)) -> String {
    let mut said = String::from("database error");
    if let Some(code) = server.code() {
        said.push(' ');
        said.push_str(&code);
    }
    let column = server
        .try_downcast_ref::<PgDatabaseError>()
        .and_then(PgDatabaseError::column);
    let named: Vec<String> = [
        ("constraint", server.constraint()),
        ("table", server.table()),
        ("column", column),
    ]
    .into_iter()
    .filter_map(|(what, name)| name.map(|name| format!("{what} {name:?}")))
    .collect();
    if !named.is_empty() {
        said.push_str(" on ");
        said.push_str(&named.join(", "));
    }
    said
}

/// What every edge without its own rendering of a `ServiceError` answers: a
/// business variant's problem, its sentence as the detail; `None` for `Db`,
/// `Masking` and `Internal`, which the edge answers opaquely, logging the cause.
impl nest_rs_core::ToProblem for ServiceError {
    fn to_problem(&self) -> Option<nest_rs_core::Problem> {
        use nest_rs_core::Problem;
        use nest_rs_core::problem::code;

        let problem = match self {
            Self::Validation(_) | Self::Invalid(_) => Problem::new(422, code::INVALID_ARGUMENT),
            Self::Conflict(_) => Problem::new(409, code::CONFLICT),
            Self::Forbidden(_) => Problem::new(403, code::FORBIDDEN),
            Self::NotFound(_) => Problem::new(404, code::NOT_FOUND),
            Self::Db(_) if self.is_unavailable() => unavailable(),
            Self::Db(_) | Self::Masking { .. } | Self::Internal { .. } => return None,
        };
        Some(problem.with_detail(self.to_string()))
    }
}

/// What a statement Postgres cancelled answers on every edge: `503`,
/// `UNAVAILABLE`, and the wait the HTTP edge's own deadline names — the budget
/// that ran out, here the statement bound of the pool the unit ran on, in whole
/// seconds rounded up. Outside a unit, or on a pool with no bound, no wait is
/// invented.
fn unavailable() -> nest_rs_core::Problem {
    use nest_rs_core::__private::retry_after_secs;
    use nest_rs_core::Problem;
    use nest_rs_core::problem::code;

    let problem = Problem::new(503, code::UNAVAILABLE);
    let bound = crate::executor::current_executor()
        .as_ref()
        .and_then(crate::executor::Executor::pool)
        .and_then(crate::module::statement_bound);
    match bound {
        Some(bound) => problem.with_retry_after(retry_after_secs(bound)),
        None => problem,
    }
}

/// `?` on a `ServiceError` inside an `async_graphql::Result` answers as the
/// resolver returning it would: through [`ToProblem`](nest_rs_core::ToProblem).
#[cfg(feature = "graphql")]
impl From<ServiceError> for nest_rs_graphql::async_graphql::Error {
    fn from(error: ServiceError) -> Self {
        nest_rs_graphql::problem_error(&error)
    }
}

/// What a `#[crud]` operation's failure answers on every edge: a unique
/// violation is a `409`, a create its scope re-check rolled back
/// (`RecordNotInserted`) a `403`, a row gone before the write a `404`, a
/// statement Postgres cancelled a `503` filed at `warn`; any other failure a
/// `500`, its error filed once at `error`.
#[cfg(any(feature = "http", feature = "graphql"))]
fn crud_problem(err: &DbErr) -> nest_rs_core::Problem {
    use nest_rs_core::Problem;
    use nest_rs_core::problem::code;

    match DbOutcome::of(err) {
        DbOutcome::Conflict => Problem::new(409, code::CONFLICT),
        DbOutcome::NotFound => Problem::new(404, code::NOT_FOUND),
        DbOutcome::Unavailable => {
            tracing::warn!(
                target: crate::target::ORM,
                kind = "db",
                error = %nest_rs_core::error_message(err),
                "crud operation unavailable",
            );
            unavailable()
        }
        DbOutcome::Internal if matches!(err, DbErr::RecordNotInserted) => {
            Problem::new(403, code::FORBIDDEN)
        }
        DbOutcome::Internal => {
            tracing::error!(
                target: crate::target::ORM,
                kind = "db",
                error = %nest_rs_core::error_message(err),
                "crud operation failed",
            );
            Problem::new(500, code::INTERNAL)
        }
    }
}

#[cfg(feature = "http")]
pub use http::crud_error as http_crud_error;

#[cfg(feature = "graphql")]
pub use graphql::crud_error as graphql_crud_error;

#[cfg(feature = "graphql")]
mod graphql {
    use nest_rs_graphql::async_graphql::Error;
    use sea_orm::DbErr;

    /// A GraphQL `#[crud]` operation's failure as the class a client branches
    /// on: the HTTP twin's statuses, as `CONFLICT`, `FORBIDDEN`, `NOT_FOUND` or
    /// the opaque `INTERNAL`. Called by the `#[crud]` expansion.
    pub fn crud_error(err: DbErr) -> Error {
        nest_rs_graphql::problem_error(&super::crud_problem(&err))
    }

    #[cfg(test)]
    mod tests {
        use nest_rs_core::OPAQUE_CLIENT_MESSAGE;
        use nest_rs_core::problem::code;
        use nest_rs_graphql::CODE_EXTENSION;

        use super::*;

        fn answered(err: DbErr) -> (String, serde_json::Value) {
            let error = crud_error(err);
            let extensions = serde_json::to_value(&error.extensions).expect("extensions serialize");
            (error.message, extensions[CODE_EXTENSION].clone())
        }

        #[test]
        fn each_class_answers_its_code_and_says_no_name_of_the_schema() {
            let logs = nest_rs_testing::LogCapture::install();

            let (said, code) = answered(crate::error::tests::taken_email());
            assert_eq!(
                (said.as_str(), code),
                ("conflict", code::CONFLICT.as_str().into())
            );
            let (said, code) = answered(DbErr::RecordNotUpdated);
            assert_eq!(
                (said.as_str(), code),
                ("not found", code::NOT_FOUND.as_str().into())
            );
            let (said, code) = answered(DbErr::RecordNotInserted);
            assert_eq!(
                (said.as_str(), code),
                ("forbidden", code::FORBIDDEN.as_str().into())
            );
            assert!(
                logs.find(crate::target::ORM, "crud operation failed")
                    .is_empty(),
                "a client's class is no incident: {:#?}",
                logs.events(),
            );
        }

        #[tokio::test]
        async fn a_cancelled_statement_tells_a_graphql_client_its_wait() {
            use crate::error::tests::{
                STATEMENT_BOUND, cancelled_statement, on_a_pool_bounding_statements,
            };

            let (crud, service) = on_a_pool_bounding_statements(STATEMENT_BOUND, || {
                (
                    crud_error(cancelled_statement()),
                    nest_rs_graphql::problem_error(&crate::ServiceError::from(
                        cancelled_statement(),
                    )),
                )
            })
            .await;
            for error in [crud, service] {
                let extensions =
                    serde_json::to_value(&error.extensions).expect("extensions serialize");
                assert_eq!(extensions[CODE_EXTENSION], code::UNAVAILABLE.as_str());
                assert_eq!(extensions["retryAfterSeconds"], 3, "{extensions}");
            }
        }

        #[test]
        fn anything_else_answers_opaquely_and_files_its_error_once_without_the_value() {
            let logs = nest_rs_testing::LogCapture::install();

            let (said, code) = answered(crate::error::tests::bad_uuid());
            assert_eq!(said, OPAQUE_CLIENT_MESSAGE);
            assert_eq!(code, code::INTERNAL.as_str());

            let event = logs.expect_one(crate::target::ORM, "crud operation failed");
            assert_eq!(event.level, "error");
            let filed = event.field("error").unwrap_or_default();
            assert!(filed.contains("22P02"), "{filed}");
            assert!(!filed.contains("secret-123"), "{filed}");
        }

        #[test]
        fn an_internal_failure_answers_opaquely_and_files_its_detail() {
            let logs = nest_rs_testing::LogCapture::install();

            let refused = std::io::Error::other("the key store refused the connection");
            let error = Error::from(crate::ServiceError::internal_because(
                "could not sign the export",
                refused,
            ));
            assert_eq!(error.message, OPAQUE_CLIENT_MESSAGE);

            let event = logs.expect_one(nest_rs_graphql::TARGET, "graphql operation failed");
            assert_eq!(
                event.field("error").as_deref(),
                Some(
                    "internal error: could not sign the export: the key store refused the connection"
                ),
            );
        }

        #[test]
        fn a_wrapped_opaque_failure_answers_the_constant_whatever_its_cause() {
            use nest_rs_graphql::__private::{ErrorReport, ErrorReportChain};

            let logs = nest_rs_testing::LogCapture::install();
            for failure in crate::error::tests::opaque_over_a_decode_failure() {
                let wrapped = anyhow::Error::from(failure).context("syncing the ledger");
                let error = ErrorReport(wrapped).into_failure().into_answer();
                assert_eq!(error.message, OPAQUE_CLIENT_MESSAGE);
                let extensions = serde_json::to_value(&error.extensions).expect("serialize");
                assert_eq!(extensions[CODE_EXTENSION], code::INTERNAL.as_str());
            }
            for event in logs.find(nest_rs_graphql::TARGET, "graphql operation failed") {
                let filed = event.field("error").unwrap_or_default();
                assert!(filed.contains("expected u64"), "{filed}");
                assert!(!filed.contains("sk_live"), "{filed}");
            }
        }
    }
}

#[cfg(feature = "http")]
mod http {
    use nest_rs_core::problem::code;
    use nest_rs_core::{Problem, ToProblem};
    use nest_rs_http::{
        HttpError, IntoResponse, ProblemDetails, Response, ResponseError, StatusCode,
    };

    use super::ServiceError;

    impl ResponseError for ServiceError {
        fn status(&self) -> StatusCode {
            match self {
                ServiceError::Validation(_) | ServiceError::Invalid(_) => {
                    StatusCode::UNPROCESSABLE_ENTITY
                }
                ServiceError::Conflict(_) => StatusCode::CONFLICT,
                ServiceError::Forbidden(_) => StatusCode::FORBIDDEN,
                ServiceError::NotFound(_) => StatusCode::NOT_FOUND,
                ServiceError::Db(_) if self.is_unavailable() => StatusCode::SERVICE_UNAVAILABLE,
                ServiceError::Db(_)
                | ServiceError::Masking { .. }
                | ServiceError::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            }
        }

        /// Render the failure as an RFC 9457 `application/problem+json`: the
        /// problem it says, with its code, its wait and a validation failure's
        /// `errors` member, or an opaque one's `500` under `INTERNAL`, as a
        /// `#[crud]` operation answers it. The cause an opaque variant
        /// withholds is the edge's to file.
        fn as_response(&self) -> Response {
            let Some(problem) = self.to_problem() else {
                return ResponseError::as_response(&Problem::new(500, code::INTERNAL));
            };
            match self.field_errors() {
                Some(fields) => ProblemDetails::from_problem(&problem)
                    .with_extension("errors", fields)
                    .into_response(),
                None => ResponseError::as_response(&problem),
            }
        }
    }

    /// A `#[crud]` operation's failure as the problem document of its class:
    /// `409` for a unique violation, `403` for a create its scope re-check
    /// rolled back, `404` for a row gone before the write, `503` for a
    /// statement Postgres cancelled, else a `500` whose error is filed at
    /// `error` — each with the `code` member the other edges answer. Called by
    /// the `#[crud]` expansion.
    pub fn crud_error(err: sea_orm::DbErr) -> HttpError {
        HttpError::from(super::crud_problem(&err))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn validation_is_422() {
            let err = ServiceError::Validation(validator::ValidationErrors::new());
            assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }

        #[test]
        fn db_is_500() {
            let err = ServiceError::Db(sea_orm::DbErr::Custom("boom".into()));
            assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
        }

        /// Rendered for the HTTP edge, which files what a `5xx` withholds
        /// once the answer is final, no variant files anything of its own.
        #[test]
        fn a_variant_rendered_for_the_edge_files_nothing_of_its_own() {
            let logs = nest_rs_testing::LogCapture::install();
            let failure =
                serde_json::from_str::<u64>(r#""secret-value""#).expect_err("a string is no u64");
            for error in [
                ServiceError::Db(crate::error::tests::bad_uuid()),
                ServiceError::Db(crate::error::tests::cancelled_statement()),
                crate::error::masking(&failure),
                ServiceError::internal("could not sign the export"),
                ServiceError::not_found("no such widget"),
            ] {
                let _ = error.as_response();
            }
            assert!(logs.events().is_empty(), "{:#?}", logs.events());
        }

        #[test]
        fn a_crud_500_emits_the_cause_and_the_mapped_statuses_do_not() {
            let logs = nest_rs_testing::LogCapture::install();

            let mapped = crud_error(sea_orm::DbErr::RecordNotInserted);
            assert_eq!(mapped.status(), StatusCode::FORBIDDEN);
            let taken = crud_error(crate::error::tests::taken_email());
            assert_eq!(taken.status(), StatusCode::CONFLICT);
            let vanished = crud_error(sea_orm::DbErr::RecordNotUpdated);
            assert_eq!(vanished.status(), StatusCode::NOT_FOUND);

            let unexpected = crud_error(sea_orm::DbErr::Custom("deadlock detected".into()));
            assert_eq!(unexpected.status(), StatusCode::INTERNAL_SERVER_ERROR);

            let event = logs.expect_one(crate::target::ORM, "crud operation failed");
            assert_eq!(event.level, "error");
            assert_eq!(event.field("kind").as_deref(), Some("db"));
            assert!(
                event.field("error").is_some_and(|d| d.contains("deadlock")),
                "only the unmapped failure is logged, and it carries its cause: {:?}",
                event.fields,
            );
        }

        /// A cancelled statement answers the class every edge names it by, as
        /// the MCP and WebSocket edges do.
        #[tokio::test]
        async fn a_crud_503_answers_its_problem_document_with_the_code() {
            let unavailable = crud_error(crate::error::tests::cancelled_statement());
            assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
            let resp = unavailable.into_response();
            assert_eq!(
                resp.headers()[nest_rs_http::header::CONTENT_TYPE],
                "application/problem+json"
            );
            let body: serde_json::Value =
                serde_json::from_slice(&resp.into_body().into_bytes().await.expect("a body"))
                    .expect("problem json");
            assert_eq!(body["status"], 503);
            assert_eq!(
                body["code"],
                nest_rs_core::problem::code::UNAVAILABLE.as_str()
            );
        }

        #[test]
        fn a_crud_500_files_the_database_error_without_the_value_it_quoted() {
            let logs = nest_rs_testing::LogCapture::install();
            let failed = crud_error(crate::error::tests::bad_uuid());
            assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);

            let event = logs.expect_one(crate::target::ORM, "crud operation failed");
            let said = event.field("error").unwrap_or_default();
            assert!(said.contains("22P02"), "{said}");
            assert!(!said.contains("secret-123"), "{said}");
        }

        #[test]
        fn business_variants_map_to_their_4xx() {
            assert_eq!(
                ServiceError::invalid("x").status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
            assert_eq!(ServiceError::conflict("x").status(), StatusCode::CONFLICT);
            assert_eq!(ServiceError::forbidden("x").status(), StatusCode::FORBIDDEN);
            assert_eq!(ServiceError::not_found("x").status(), StatusCode::NOT_FOUND);
            assert_eq!(
                ServiceError::internal("x").status(),
                StatusCode::INTERNAL_SERVER_ERROR
            );
        }

        async fn body_json(err: ServiceError) -> (StatusCode, Option<String>, serde_json::Value) {
            body_of(err.as_response()).await
        }

        async fn body_json_of(err: HttpError) -> (StatusCode, Option<String>, serde_json::Value) {
            body_of(err.into_response()).await
        }

        async fn body_of(resp: Response) -> (StatusCode, Option<String>, serde_json::Value) {
            let status = resp.status();
            let ct = resp
                .headers()
                .get(nest_rs_http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let bytes = resp.into_body().into_bytes().await.expect("body");
            (
                status,
                ct,
                serde_json::from_slice(&bytes).expect("problem json"),
            )
        }

        #[tokio::test]
        async fn conflict_renders_problem_json_with_the_client_message() {
            let (status, ct, body) =
                body_json(ServiceError::conflict("insufficient credit balance")).await;
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(ct.as_deref(), Some("application/problem+json"));
            assert_eq!(body["status"], 409);
            assert_eq!(body["title"], "Conflict");
            assert_eq!(body["detail"], "insufficient credit balance");
        }

        /// An opaque failure answers the document a `#[crud]` operation
        /// answers for the same class: its code, and no detail.
        #[tokio::test]
        async fn db_error_renders_a_500_problem_without_leaking_the_driver_detail() {
            let (status, ct, body) = body_json(ServiceError::Db(sea_orm::DbErr::Custom(
                "SELECT password_hash FROM user".into(),
            )))
            .await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(ct.as_deref(), Some("application/problem+json"));
            assert_eq!(body["status"], 500);
            assert_eq!(body["code"], code::INTERNAL.as_str(), "{body}");
            assert!(body.get("detail").is_none(), "a 5xx says no detail: {body}");
            let (_, _, crud) = body_json_of(crud_error(sea_orm::DbErr::Custom(
                "SELECT password_hash FROM user".into(),
            )))
            .await;
            assert_eq!(body, crud);
            let (_, _, opaque) = body_json_of(
                nest_rs_http::Opaque::opaque(Err::<(), _>(std::io::Error::other("refused")))
                    .expect_err("a failure"),
            )
            .await;
            assert_eq!(body, opaque, "one document for the class, whoever answered");
            assert!(
                !body.to_string().contains("password_hash"),
                "the driver message must not reach the wire: {body}",
            );
        }

        #[tokio::test]
        async fn an_internal_500_answers_its_code_and_withholds_its_detail_and_cause() {
            let refused = std::io::Error::other("the key store refused the connection");
            let (status, _, body) = body_json(ServiceError::internal_because(
                "could not sign the export",
                refused,
            ))
            .await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(body["code"], code::INTERNAL.as_str(), "{body}");
            assert!(body.get("detail").is_none(), "{body}");
            assert!(!body.to_string().contains("key store"), "{body}");
        }

        /// A cancelled statement answers the class every edge names it by and,
        /// like the edge's own deadline, the bound that ran out as its wait;
        /// `#[crud]`, whose problem keeps no chain, says it at `warn`.
        #[tokio::test]
        async fn a_statement_cancelled_at_its_bound_renders_a_503() {
            use crate::error::tests::{
                STATEMENT_BOUND, cancelled_statement, on_a_pool_bounding_statements,
            };

            let logs = nest_rs_testing::LogCapture::install();
            let (service, crud) = on_a_pool_bounding_statements(STATEMENT_BOUND, || {
                (
                    ServiceError::from(cancelled_statement()).as_response(),
                    crud_error(cancelled_statement()).into_response(),
                )
            })
            .await;
            for resp in [service, crud] {
                assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
                assert_eq!(resp.headers()[nest_rs_http::header::RETRY_AFTER], "3");
                let body: serde_json::Value =
                    serde_json::from_slice(&resp.into_body().into_bytes().await.expect("a body"))
                        .expect("problem json");
                assert_eq!(body["status"], 503);
                assert_eq!(
                    body["code"],
                    nest_rs_core::problem::code::UNAVAILABLE.as_str()
                );
                assert!(body.get("detail").is_none(), "a 5xx says no detail: {body}");
            }
            let crud = logs.expect_one(crate::target::ORM, "crud operation unavailable");
            assert_eq!(crud.level, "warn");
            assert!(
                crud.field("error")
                    .is_some_and(|said| said.contains("57014")),
                "{crud:?}"
            );
            let filed = logs
                .events()
                .into_iter()
                .filter(|event| event.level == "warn" || event.level == "error")
                .count();
            assert_eq!(filed, 1, "the one line is #[crud]'s: {:#?}", logs.events());
        }

        /// A business variant answers its code beside its sentence, as a
        /// `#[crud]` operation's and every other edge's do.
        #[tokio::test]
        async fn a_business_variant_answers_its_code() {
            for (error, code) in [
                (
                    ServiceError::conflict("taken"),
                    nest_rs_core::problem::code::CONFLICT,
                ),
                (
                    ServiceError::NotFound("post".into()),
                    nest_rs_core::problem::code::NOT_FOUND,
                ),
            ] {
                let (_, _, body) = body_json(error).await;
                assert_eq!(body["code"], code.as_str(), "{body}");
            }
        }

        #[tokio::test]
        async fn validation_rides_field_errors_as_an_extension_member() {
            let mut errs = validator::ValidationErrors::new();
            errs.add("email", validator::ValidationError::new("not_an_email"));
            let (status, ct, body) = body_json(ServiceError::Validation(errs)).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(ct.as_deref(), Some("application/problem+json"));
            assert!(
                body.get("errors").and_then(|e| e.get("email")).is_some(),
                "field errors ride as the `errors` extension member: {body}",
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use nest_rs_core::error_message;
    use sea_orm::sqlx::error::{DatabaseError, ErrorKind};
    use sea_orm::{RuntimeErr, SqlxError};

    use super::*;

    /// A server's refusal as sqlx hands it over: a message quoting the row,
    /// the SQLSTATE, and the names Postgres reports beside it.
    #[derive(Debug)]
    struct ServerRefusal {
        code: &'static str,
        unique: bool,
        constraint: Option<&'static str>,
        table: Option<&'static str>,
        message: &'static str,
    }

    impl std::fmt::Display for ServerRefusal {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.message)
        }
    }

    impl std::error::Error for ServerRefusal {}

    impl DatabaseError for ServerRefusal {
        fn message(&self) -> &str {
            self.message
        }
        fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
            Some(self.code.into())
        }
        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
        fn constraint(&self) -> Option<&str> {
            self.constraint
        }
        fn table(&self) -> Option<&str> {
            self.table
        }
        fn kind(&self) -> ErrorKind {
            if self.unique {
                ErrorKind::UniqueViolation
            } else {
                ErrorKind::Other
            }
        }
    }

    fn refused(refusal: ServerRefusal) -> DbErr {
        DbErr::Exec(RuntimeErr::SqlxError(Arc::new(SqlxError::Database(
            Box::new(refusal),
        ))))
    }

    pub(super) fn taken_email() -> DbErr {
        refused(ServerRefusal {
            code: "23505",
            unique: true,
            constraint: Some("users_email_key"),
            table: Some("users"),
            message: "duplicate key value violates unique constraint: (email)=(ada@example.com)",
        })
    }

    #[test]
    fn an_atomic_block_the_database_refused_is_classified_like_its_dberr() {
        let conflict = ServiceError::from(AtomicError::Database(Box::new(taken_email())));
        assert!(
            matches!(conflict, ServiceError::Conflict(_)),
            "{conflict:?}"
        );

        let opaque = ServiceError::from(AtomicError::Database(Box::new(bad_uuid())));
        let ServiceError::Db(err) = opaque else {
            panic!("an unclassified refusal stays a database error: {opaque:?}");
        };
        assert!(!error_message(&err).contains("secret-123"));
    }

    #[test]
    fn every_other_atomic_failure_is_internal_and_keeps_it_as_its_cause() {
        for (err, said) in [
            (
                AtomicError::Poisoned { retryable: false },
                "a statement failed inside the atomic block; nothing it wrote stands",
            ),
            (
                AtomicError::Escaped,
                "the atomic block's executor outlived it",
            ),
            (
                AtomicError::Unsupported,
                "no ambient executor offers an atomic block",
            ),
            (
                AtomicError::Database(Box::new(std::io::Error::other("savepoint refused"))),
                "the atomic block could not be settled: savepoint refused",
            ),
        ] {
            let internal = ServiceError::from(err);
            assert!(
                matches!(internal, ServiceError::Internal { .. }),
                "an atomic failure is opaque to the client: {internal:?}"
            );
            assert!(
                internal
                    .source()
                    .and_then(Error::source)
                    .is_some_and(|cause| cause.is::<AtomicError>())
            );
            assert_eq!(
                error_message(&internal),
                format!("internal error: the atomic block failed: {said}")
            );
        }
    }

    #[test]
    fn an_atomic_block_over_a_dberr_body_answers_a_dberr() {
        let refused = DbErr::from(AtomicDbError::Atomic(AtomicError::Database(Box::new(
            DbErr::RecordNotUpdated,
        ))));
        assert!(matches!(refused, DbErr::RecordNotUpdated), "{refused:?}");

        let poisoned = DbErr::from(AtomicDbError::Atomic(AtomicError::Poisoned {
            retryable: true,
        }));
        assert!(
            matches!(&poisoned, DbErr::Custom(said) if said.contains("inside the atomic block")),
            "{poisoned:?}"
        );

        let own = DbErr::from(AtomicDbError::Db(DbErr::RecordNotInserted));
        assert!(matches!(own, DbErr::RecordNotInserted));
    }

    pub(super) fn bad_uuid() -> DbErr {
        refused(ServerRefusal {
            code: "22P02",
            unique: false,
            constraint: None,
            table: None,
            message: "invalid input syntax for type uuid: \"secret-123\"",
        })
    }

    /// Postgres cancelling a statement past its `statement_timeout`.
    pub(crate) fn cancelled_statement() -> DbErr {
        refused(ServerRefusal {
            code: "57014",
            unique: false,
            constraint: None,
            table: None,
            message: "canceling statement due to statement timeout",
        })
    }

    #[test]
    fn a_statement_cancelled_at_its_bound_is_the_server_unable_to_serve_in_time() {
        assert_eq!(
            DbOutcome::of(&cancelled_statement()),
            DbOutcome::Unavailable
        );
        let error = ServiceError::from(cancelled_statement());
        assert!(matches!(error, ServiceError::Db(_)), "{error:?}");
        let problem = nest_rs_core::ToProblem::to_problem(&error)
            .expect("an unavailable answer, not an opaque one");
        assert_eq!(
            (problem.status(), problem.code()),
            (503, nest_rs_core::problem::code::UNAVAILABLE)
        );
        assert!(
            crate::retry::is_transient_failure(&cancelled_statement()),
            "a fresh attempt may meet the statement in its bound"
        );
    }

    /// `read` under a unit's ambient executor over a pool whose statements
    /// Postgres cancels past `bound`, which a cancelled statement's wait is
    /// read off. Nothing is dialled.
    pub(crate) async fn on_a_pool_bounding_statements<T>(
        bound: std::time::Duration,
        read: impl FnOnce() -> T,
    ) -> T {
        let mut options = sea_orm::ConnectOptions::new("postgres://nestrs@127.0.0.1:1/nowhere");
        options.connect_lazy(true).statement_timeout(bound);
        let pool = sea_orm::Database::connect(options)
            .await
            .expect("a lazy pool dials nothing");
        crate::executor::with_executor(crate::executor::Executor::Pool(pool), async move { read() })
            .await
    }

    /// The bound every edge's test below runs its unit's pool under: a wait of
    /// three whole seconds, rounded up.
    pub(crate) const STATEMENT_BOUND: std::time::Duration = std::time::Duration::from_millis(2500);

    /// A statement Postgres cancelled waits the bound that cut it, as the HTTP
    /// edge's deadline names the budget that ran out; outside a unit no wait is
    /// invented.
    #[tokio::test]
    async fn a_cancelled_statement_waits_the_bound_of_the_pool_it_ran_on() {
        let problem = on_a_pool_bounding_statements(STATEMENT_BOUND, || {
            nest_rs_core::ToProblem::to_problem(&ServiceError::from(cancelled_statement()))
        })
        .await
        .expect("an unavailable answer");
        assert_eq!(
            (problem.status(), problem.code(), problem.retry_after()),
            (503, nest_rs_core::problem::code::UNAVAILABLE, Some(3)),
        );
        let outside =
            nest_rs_core::ToProblem::to_problem(&ServiceError::from(cancelled_statement()))
                .expect("an unavailable answer");
        assert_eq!(outside.retry_after(), None);
    }

    #[tokio::test]
    async fn a_cancelled_statement_tells_a_websocket_client_its_wait() {
        let reply = on_a_pool_bounding_statements(STATEMENT_BOUND, || {
            nest_rs_ws::ErrorReport::new(ServiceError::from(cancelled_statement()))
                .into_failure()
                .into_reply("sync")
        })
        .await;
        let nest_rs_ws::WsReply::Error(frame) = reply else {
            panic!("a handler's error answers an error frame");
        };
        assert_eq!(
            frame.errors,
            Some(serde_json::json!({ "code": "UNAVAILABLE", "retryAfterSeconds": 3 })),
        );
    }

    #[tokio::test]
    async fn a_cancelled_statement_tells_an_mcp_client_its_wait() {
        let error = on_a_pool_bounding_statements(STATEMENT_BOUND, || {
            nest_rs_mcp::problem_error(&ServiceError::from(cancelled_statement()))
        })
        .await;
        assert_eq!(
            error.data,
            Some(serde_json::json!({ "code": "UNAVAILABLE", "retryAfterSeconds": 3 })),
        );
    }

    #[test]
    fn a_database_failure_is_classified_once_off_the_error_alone() {
        assert_eq!(DbOutcome::of(&taken_email()), DbOutcome::Conflict);
        assert_eq!(DbOutcome::of(&DbErr::RecordNotUpdated), DbOutcome::NotFound);
        assert_eq!(
            DbOutcome::of(&DbErr::RecordNotFound("widget".into())),
            DbOutcome::NotFound
        );
        // sea-orm raises it for an `ON CONFLICT DO NOTHING` that inserted nothing too.
        assert_eq!(
            DbOutcome::of(&DbErr::RecordNotInserted),
            DbOutcome::Internal
        );
        assert_eq!(
            DbOutcome::of(&DbErr::Custom("boom".into())),
            DbOutcome::Internal
        );
        assert_eq!(DbOutcome::of(&bad_uuid()), DbOutcome::Internal);
    }

    #[test]
    fn a_unique_violation_is_a_conflict_and_a_vanished_row_is_not_found() {
        match ServiceError::from(taken_email()) {
            ServiceError::Conflict(said) => {
                assert_eq!(said, "the request conflicts with an existing record");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        assert!(matches!(
            ServiceError::from(DbErr::RecordNotUpdated),
            ServiceError::NotFound(_)
        ));
        assert!(matches!(
            ServiceError::from(bad_uuid()),
            ServiceError::Db(_)
        ));
    }

    #[test]
    fn a_server_error_is_said_by_its_code_and_the_names_it_reports() {
        assert_eq!(
            error_message(&taken_email()),
            "Execution Error: database error 23505 on constraint \"users_email_key\", table \"users\""
        );
        let said = error_message(&bad_uuid());
        assert_eq!(said, "Execution Error: database error 22P02");
    }

    #[test]
    fn a_driver_message_quoting_a_value_never_reaches_the_sentence() {
        let quoting = [
            DbErr::Exec(RuntimeErr::Internal("value \"secret\"".into())),
            DbErr::Query(RuntimeErr::Internal("value \"secret\"".into())),
            DbErr::Type("could not read \"secret\" as an integer".into()),
            DbErr::Json("expected an object at \"secret\"".into()),
            DbErr::TryIntoErr {
                from: "String",
                into: "Uuid",
                source: Arc::new(std::io::Error::other("\"secret\" is no uuid")),
            },
            bad_uuid(),
            taken_email(),
        ];
        for err in quoting {
            let said = error_message(&err);
            assert!(!said.contains("secret") && !said.contains("ada@"), "{said}");
            let service = error_message(&ServiceError::Db(err));
            assert!(
                !service.contains("secret") && !service.contains("ada@"),
                "{service}"
            );
        }
    }

    #[test]
    fn a_database_error_inside_any_wrapper_is_said_the_same_way() {
        #[derive(Debug, thiserror::Error)]
        #[error("{0}")]
        struct JobShaped(#[source] Box<dyn std::error::Error + Send + Sync>);

        let wrapped = [
            nest_rs_core::boxed_error(anyhow::Error::from(bad_uuid()).context("the job failed")),
            anyhow::Error::from(bad_uuid())
                .context("the job failed")
                .into(),
            anyhow::Error::from(bad_uuid()).into(),
            Box::new(JobShaped(nest_rs_core::boxed_error(anyhow::Error::from(
                bad_uuid(),
            )))),
            Box::new(CommitError(bad_uuid())),
        ];
        for error in wrapped {
            let said = error_message(&*error);
            assert!(!said.contains("secret-123"), "{said}");
            assert!(said.contains("database error 22P02"), "{said}");
            let displayed = error.to_string();
            let redacted = nest_rs_core::DecodeError::redact(&displayed, Some(&*error));
            assert!(!redacted.contains("secret-123"), "{redacted}");
        }
    }

    #[test]
    fn what_carries_no_value_keeps_its_own_sentence() {
        for err in [
            DbErr::RecordNotFound("widget 7".into()),
            DbErr::RecordNotInserted,
            DbErr::RecordNotUpdated,
            DbErr::Custom("cursor column is not a uuid".into()),
            DbErr::ConnectionAcquire(sea_orm::ConnAcquireErr::Timeout),
            DbErr::AttrNotSet("users.email".into()),
            DbErr::Conn(RuntimeErr::SqlxError(Arc::new(SqlxError::PoolTimedOut))),
        ] {
            assert_eq!(error_message(&err), err.to_string());
        }
        let refused = DbErr::Conn(RuntimeErr::SqlxError(Arc::new(SqlxError::Io(
            std::io::Error::from(std::io::ErrorKind::ConnectionRefused),
        ))));
        assert_eq!(
            error_message(&refused),
            "Connection Error: error communicating with database: connection refused"
        );
    }

    #[test]
    fn db_display_is_wire_safe_constant() {
        let err = ServiceError::Db(DbErr::Custom("SELECT password_hash FROM user".into()));
        assert_eq!(err.to_string(), "database error");
    }

    #[test]
    fn internal_display_is_wire_safe_constant() {
        for err in [
            ServiceError::internal("stripe key rejected: sk_live_123"),
            ServiceError::internal_because(
                "stripe key rejected: sk_live_123",
                std::io::Error::other("refused"),
            ),
        ] {
            assert_eq!(err.to_string(), nest_rs_core::OPAQUE_CLIENT_MESSAGE);
        }
    }

    #[test]
    fn an_internal_failure_keeps_its_detail_and_cause_and_stays_clone() {
        let refused = std::io::Error::other("the key store refused the connection");
        let err = ServiceError::internal_because("could not sign the export", refused);
        let copy = err.clone();
        for err in [&err, &copy] {
            let detail = err.source().expect("the detail is the source");
            assert_eq!(detail.to_string(), "could not sign the export");
            let cause = detail.source().expect("the cause is the detail's source");
            assert!(cause.is::<std::io::Error>(), "{cause:?}");
            assert_eq!(
                error_message(err),
                "internal error: could not sign the export: the key store refused the connection"
            );
        }
        let bare = ServiceError::internal("could not sign the export");
        assert_eq!(
            error_message(&bare),
            "internal error: could not sign the export"
        );
        assert!(bare.source().and_then(Error::source).is_none());
    }

    /// Each opaque failure whose chain holds an upstream's decode failure.
    pub(super) fn opaque_over_a_decode_failure() -> [ServiceError; 3] {
        let upstream =
            || serde_json::from_str::<u64>(r#""sk_live_secret""#).expect_err("not a number");
        [
            ServiceError::internal_because("the upstream reply did not decode", upstream()),
            ServiceError::internal_because(
                "the upstream reply did not decode",
                anyhow::Error::from(upstream()).context("reading the reply"),
            ),
            masking(&upstream()),
        ]
    }

    #[test]
    fn a_wrapped_opaque_failure_answers_a_websocket_client_the_constant_whatever_its_cause() {
        let logs = nest_rs_testing::LogCapture::install();
        for failure in opaque_over_a_decode_failure() {
            let wrapped = anyhow::Error::from(failure).context("syncing the ledger");
            let nest_rs_ws::WsReply::Error(frame) =
                nest_rs_ws::WsReply::from_handler_error("sync", wrapped)
            else {
                panic!("a handler's error answers an error frame");
            };
            assert_eq!(frame.error, nest_rs_core::OPAQUE_CLIENT_MESSAGE);
        }
        let filed = logs.find(nest_rs_ws::TARGET, "websocket message failed");
        assert_eq!(filed.len(), 3, "{:#?}", logs.events());
        for event in filed {
            let said = event.field("error").unwrap_or_default();
            assert!(said.contains("expected u64"), "{said}");
            assert!(!said.contains("sk_live"), "{said}");
        }
    }

    #[test]
    fn a_cause_quoting_a_value_is_said_without_it() {
        let quoting =
            || serde_json::from_str::<u64>(r#""sk_live_secret""#).expect_err("not a number");
        for err in [
            ServiceError::internal_because("the upstream reply did not decode", quoting()),
            ServiceError::internal_because(
                "the upstream reply did not decode",
                anyhow::Error::from(quoting()).context("reading the reply"),
            ),
            masking(&quoting()),
        ] {
            let said = error_message(&err);
            assert!(!said.contains("sk_live"), "{said}");
            assert!(said.contains("expected u64"), "{said}");
        }
    }

    #[test]
    fn business_variants_forward_their_message() {
        assert_eq!(
            ServiceError::conflict("insufficient credit balance").to_string(),
            "insufficient credit balance"
        );
    }

    #[test]
    fn db_from_db_err_does_not_lose_inner() {
        let inner = DbErr::Custom("connection lost".into());
        let err: ServiceError = inner.into();
        match err {
            ServiceError::Db(DbErr::Custom(msg)) => assert_eq!(msg, "connection lost"),
            other => panic!("expected Db, got {other:?}"),
        }
    }

    #[test]
    fn validation_from_validation_errors_propagates_field_errors() {
        let mut errs = ValidationErrors::new();
        errs.add("email", validator::ValidationError::new("not_an_email"));
        let err: ServiceError = errs.into();
        match err {
            ServiceError::Validation(v) => assert!(v.field_errors().contains_key("email")),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn field_errors_never_echo_the_submitted_value() {
        let mut error = validator::ValidationError::new("length");
        error.add_param("min".into(), &1);
        error.add_param("value".into(), &"hunter2");
        let mut errors = validator::ValidationErrors::new();
        errors.add("password", error);

        let fields = ServiceError::Validation(errors)
            .field_errors()
            .expect("a validation failure carries its fields");
        let rendered = fields.to_string();

        assert!(rendered.contains("length"), "the rule survives: {rendered}");
        assert!(
            !rendered.contains("params"),
            "none of its parameters: {rendered}"
        );
        assert!(
            !rendered.contains("hunter2"),
            "the submitted value must never ride back out: {rendered}",
        );
    }

    #[test]
    fn a_business_variant_says_its_problem_and_an_opaque_one_says_none() {
        use nest_rs_core::ToProblem;
        use nest_rs_core::problem::code;

        let said = |error: ServiceError| {
            error
                .to_problem()
                .map(|problem| (problem.status(), problem.code(), problem.client_message()))
        };
        assert_eq!(
            said(ServiceError::not_found("no such widget")),
            Some((404, code::NOT_FOUND, "no such widget".into()))
        );
        assert_eq!(
            said(ServiceError::conflict("closed")),
            Some((409, code::CONFLICT, "closed".into()))
        );
        assert_eq!(
            said(ServiceError::forbidden("not yours")),
            Some((403, code::FORBIDDEN, "not yours".into()))
        );
        assert_eq!(
            said(ServiceError::invalid("empty")),
            Some((422, code::INVALID_ARGUMENT, "empty".into()))
        );
        assert_eq!(
            said(ServiceError::Validation(validator::ValidationErrors::new())),
            Some((422, code::INVALID_ARGUMENT, "validation failed".into()))
        );
        for opaque in [
            ServiceError::Db(DbErr::Custom("relation \"posts\" does not exist".into())),
            masking(&serde_json::from_str::<u64>(r#""row 7""#).expect_err("not a number")),
            ServiceError::internal("hash failed"),
        ] {
            assert_eq!(opaque.to_problem(), None, "{opaque:?}");
        }
    }
}

/// A commit-time database failure, opaque over `DbErr` so a sea-orm bump is not
/// a semver break through [`FinalizeOutcome`](crate::FinalizeOutcome).
#[derive(Debug)]
pub struct CommitError(pub(crate) DbErr);

impl CommitError {
    /// Whether the commit failed on a retryable serialization/deadlock conflict
    /// (a typed SQLSTATE `40001`/`40P01`/…).
    pub fn is_retryable_conflict(&self) -> bool {
        crate::retry::is_retryable_conflict(&self.0)
    }
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for CommitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// Why the boot could not open the pool. Postgres is named by its host and
/// port, never by the URL, which carries the password; what the server said
/// travels as the source, said by its SQLSTATE.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConnectError {
    /// Postgres refused what every attempt would send again.
    #[error("Postgres at {endpoint} refused the connection: {reason}")]
    Refused {
        endpoint: String,
        reason: String,
        #[source]
        source: DbErr,
    },
    /// No attempt opened a connection within the connect budget.
    #[error(
        "Postgres at {endpoint} could not be reached within {budget:?}, after {attempts} \
         attempt(s): check that it runs and that {url} names it, or widen {timeout}"
    )]
    Unreachable {
        endpoint: String,
        budget: std::time::Duration,
        attempts: u32,
        url: String,
        timeout: String,
        #[source]
        source: Option<DbErr>,
    },
}

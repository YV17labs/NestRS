//! `ServiceError` stays unprefixed: it is written in every app service's
//! signature, like `Service` itself.
//!
//! A database failure is classified once, here ([`DbOutcome`]), and said once:
//! this crate's [`SafeLink`](nest_rs_core::SafeLink) says a sea-orm or sqlx
//! error by its SQLSTATE and the names the server reported, never by the
//! server's message, which quotes the input it refused.

use sea_orm::sqlx::error::DatabaseError;
use sea_orm::sqlx::postgres::PgDatabaseError;
use sea_orm::{DbErr, RuntimeErr, SqlxError};
use validator::ValidationErrors;

/// Failure modes shared by every service method. The business variants carry a
/// client-facing message and map to their 4xx; the opaque ones (`Db`,
/// `Internal`, `Masking`) put a constant on the wire and keep the detail for
/// `tracing`.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum ServiceError {
    /// Edge validation (`validator`) rejected the input — maps to a 422. The
    /// fields ride under `errors` ([`field_errors`](Self::field_errors)), never
    /// `validator`'s `Debug`, which echoes the rejected value.
    #[error("validation failed")]
    Validation(#[from] ValidationErrors),
    /// A `Repo`/ORM query failed for a reason the caller cannot act on
    /// ([`DbOutcome::Internal`]). The `DbErr` stays for `tracing`, said by its
    /// SQLSTATE; the wire sees a generic message.
    #[error("database error")]
    Db(#[source] DbErr),
    /// Response masking could not reconcile a loaded row into its wire DTO: a
    /// 500. A `String`, not the `serde_json::Error`, so the enum stays `Clone`
    /// for dataloaders.
    #[error("response masking failed")]
    Masking(String),
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
    /// upstream call). Maps to **500**; like `Db`, the detail stays for
    /// `tracing` and the wire sees a constant string.
    #[error("{}", nest_rs_core::OPAQUE_CLIENT_MESSAGE)]
    Internal(String),
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

    /// **500** — an internal failure that is not a database error. The detail is
    /// kept for `tracing` only; the wire sees a constant string.
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
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
}

/// A `DbErr` classified by [`DbOutcome`]: a unique violation is a
/// [`Conflict`](ServiceError::Conflict) (`409`), a row a write found gone is
/// [`NotFound`](ServiceError::NotFound) (`404`), anything else is
/// [`Db`](ServiceError::Db) (`500`).
impl From<DbErr> for ServiceError {
    fn from(err: DbErr) -> Self {
        match DbOutcome::of(&err) {
            DbOutcome::Conflict => {
                Self::Conflict("the request conflicts with an existing record".into())
            }
            DbOutcome::NotFound => Self::NotFound("not found".into()),
            DbOutcome::Internal => Self::Db(err),
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
            _ if server_error(err).is_some_and(|server| server.is_unique_violation()) => {
                Self::Conflict
            }
            _ => Self::Internal,
        }
    }
}

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
            Self::Db(_) | Self::Masking(_) | Self::Internal(_) => return None,
        };
        Some(problem.with_detail(self.to_string()))
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
/// (`RecordNotInserted`) a `403`, a row gone before the write a `404`; any other
/// failure a `500`, its error filed once at `error`.
#[cfg(any(feature = "http", feature = "graphql"))]
fn crud_problem(err: &DbErr) -> nest_rs_core::Problem {
    use nest_rs_core::Problem;
    use nest_rs_core::problem::code;

    match DbOutcome::of(err) {
        DbOutcome::Conflict => Problem::new(409, code::CONFLICT),
        DbOutcome::NotFound => Problem::new(404, code::NOT_FOUND),
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
pub use http::crud_error;

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
    }
}

#[cfg(feature = "http")]
mod http {
    use std::borrow::Cow;

    use nest_rs_http::ProblemDetails;
    use poem::error::ResponseError;
    use poem::http::StatusCode;
    use poem::{IntoResponse, Response};

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
                ServiceError::Db(_) | ServiceError::Masking(_) | ServiceError::Internal(_) => {
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }
        }

        /// Render the failure as an RFC 9457 `application/problem+json` whose
        /// `detail` is the wire-safe `Display`; the one place an opaque
        /// variant's cause is logged.
        fn as_response(&self) -> Response {
            let status = self.status();
            log_opaque(self);
            let mut problem = ProblemDetails::from_status(status).with_detail(self.to_string());
            if let Some(fields) = self.field_errors() {
                problem = problem.with_extension("errors", fields);
            }
            problem.into_response()
        }
    }

    /// Emit the cause behind an opaque 5xx; a 4xx stays silent.
    fn log_opaque(err: &ServiceError) {
        let (kind, said): (&str, Cow<'_, str>) = match err {
            ServiceError::Db(e) => ("db", nest_rs_core::error_message(e).into()),
            ServiceError::Masking(e) => ("masking", e.into()),
            ServiceError::Internal(e) => ("internal", e.into()),
            _ => return,
        };
        tracing::error!(
            target: crate::target::ORM,
            kind,
            error = %said,
            "service error surfaced as 500",
        );
    }

    /// A `#[crud]` operation's failure as its status, with an empty body: `409`
    /// for a unique violation, `403` for a create its scope re-check rolled
    /// back, `404` for a row gone before the write, else a `500` whose error is
    /// filed at `error`. Called by the `#[crud]` expansion.
    pub fn crud_error(err: sea_orm::DbErr) -> poem::Error {
        let problem = super::crud_problem(&err);
        poem::Error::from_status(nest_rs_http::ResponseError::status(&problem))
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

        #[test]
        fn a_500_emits_the_cause_it_refuses_to_ship() {
            let logs = nest_rs_testing::LogCapture::install();
            let _ = ServiceError::Db(sea_orm::DbErr::Custom(
                "relation \"posts\" does not exist".into(),
            ))
            .as_response();

            let event = logs.expect_one(crate::target::ORM, "service error surfaced as 500");
            assert_eq!(event.level, "error");
            assert_eq!(event.field("kind").as_deref(), Some("db"));
            assert!(
                event.field("error").is_some_and(|d| d.contains("posts")),
                "the cause the body withholds is the whole point, got {:?}",
                event.fields,
            );
        }

        #[test]
        fn a_500_files_the_database_error_without_the_value_it_quoted() {
            let logs = nest_rs_testing::LogCapture::install();
            let _ = ServiceError::Db(crate::error::tests::bad_uuid()).as_response();

            let event = logs.expect_one(crate::target::ORM, "service error surfaced as 500");
            let said = event.field("error").unwrap_or_default();
            assert!(said.contains("22P02"), "the SQLSTATE stays: {said}");
            assert!(!said.contains("secret-123"), "the input goes: {said}");
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
        fn a_business_4xx_emits_nothing() {
            let logs = nest_rs_testing::LogCapture::install();
            let _ = ServiceError::not_found("no such widget").as_response();
            assert!(
                logs.find(crate::target::ORM, "service error surfaced as 500")
                    .is_empty(),
                "a 404 is not an incident: {:#?}",
                logs.events(),
            );
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
            let resp = err.as_response();
            let status = resp.status();
            let ct = resp
                .headers()
                .get(poem::http::header::CONTENT_TYPE)
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

        #[tokio::test]
        async fn db_error_renders_a_500_problem_without_leaking_the_driver_detail() {
            let (status, ct, body) = body_json(ServiceError::Db(sea_orm::DbErr::Custom(
                "SELECT password_hash FROM user".into(),
            )))
            .await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(ct.as_deref(), Some("application/problem+json"));
            assert_eq!(body["status"], 500);
            assert_eq!(body["detail"], "database error");
            assert!(
                !body.to_string().contains("password_hash"),
                "the driver message must not reach the wire: {body}",
            );
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
mod tests {
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

    pub(super) fn bad_uuid() -> DbErr {
        refused(ServerRefusal {
            code: "22P02",
            unique: false,
            constraint: None,
            table: None,
            message: "invalid input syntax for type uuid: \"secret-123\"",
        })
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
        let err = ServiceError::internal("stripe key rejected: sk_live_… ");
        assert_eq!(err.to_string(), nest_rs_core::OPAQUE_CLIENT_MESSAGE);
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
            ServiceError::Masking("row 7".into()),
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

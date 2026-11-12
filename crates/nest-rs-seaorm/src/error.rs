//! `ServiceError` stays unprefixed: it is written in every app service's
//! signature, like `Service` itself.
//!
//! A database failure is classified once, here ([`RepoError`]), and said once:
//! by its SQLSTATE and the names the server reported, never by the server's
//! message or its `detail`, which quote the input it refused. This crate's
//! [`SafeLink`](nest_rs_core::SafeLink) says a sea-orm or sqlx error a chain
//! still holds the same way.

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use nest_rs_core::__private::OpaqueCause;
use nest_rs_core::DecodeError;
use nest_rs_database::AtomicError;
use ref_cast::RefCast;
use sea_orm::sqlx::error::{DatabaseError, ErrorKind};
use sea_orm::sqlx::postgres::PgDatabaseError;
use sea_orm::{DbErr, RuntimeErr, SqlxError};
use validator::ValidationErrors;

/// What a unit lacks to load the subject `#[authorize(A, bind = S)]` names: a
/// wiring defect, never the caller's doing.
#[derive(Debug, thiserror::Error)]
pub(crate) enum UnwiredBind {
    /// Nothing attached an ability to the unit.
    #[error("no ability on the unit — is the ability guard applied to this site?")]
    Ability,
    /// The edge opened the unit outside a request scope.
    #[error("no request scope on the unit — the transport edge must wrap the site")]
    RequestScope,
    /// The bound service is no provider of a module the app imports.
    #[error("no provider for the bound service — add it to a module's providers")]
    Provider,
}

/// Failure modes shared by every service method. The business variants carry a
/// client-facing message and map to their 4xx; the opaque ones (`Db`,
/// `Internal`) answer a constant on every edge and keep their cause,
/// said through [`error_message`](fn@nest_rs_core::error_message), for the
/// operator's line. A `{:?}`, an `anyhow` report and a `main` returning it read
/// what that says too: the value a rule refused stays in the variant, for a
/// `match`, a database failure is a [`RepoError`], and an internal failure's
/// cause stays beneath it.
#[derive(Clone)]
#[non_exhaustive]
pub enum ServiceError {
    /// The input breaks the rules its type declares (`validator`) — maps to a
    /// **400**, as the edge's own refusal of that value does, whoever checks
    /// it. The fields ride under `errors` ([`field_errors`](Self::field_errors)),
    /// never `validator`'s `Debug`, which echoes the rejected value.
    Validation(ValidationErrors),
    /// A `Repo` statement failed for a reason the caller cannot act on (a
    /// `500`), or Postgres cancelled it at its bound
    /// ([`RepoError::QueryCanceled`], a `503`). The failure stays for a `match`
    /// and the operator's line, said by its SQLSTATE; the wire sees a generic
    /// message.
    Db(RepoError),
    /// A well-formed request the service rejects on business grounds (empty
    /// body, non-positive amount). Maps to **422**; the message is client-facing.
    Invalid(String),
    /// The action conflicts with the resource's current state (spending past a
    /// balance, acting on a closed record). Maps to **409**; client-facing.
    Conflict(String),
    /// The caller is known but not permitted to perform this action. Maps to
    /// **403**; client-facing.
    Forbidden(String),
    /// The addressed resource does not exist (or is deliberately hidden from
    /// this caller). Maps to **404**; client-facing.
    NotFound(String),
    /// An internal failure that is not the database's (a hash, an enqueue push,
    /// an upstream call). Maps to **500**: it displays the constant every edge
    /// answers, and its [`source`](Error::source) says the detail, then the
    /// cause's chain, in one sentence, for the operator's line. Built by
    /// [`internal`](Self::internal) or, with what failed,
    /// [`internal_because`](Self::internal_because).
    #[non_exhaustive]
    Internal {
        /// The detail and its cause, shared so the error stays `Clone` for
        /// dataloaders; read through [`Error::source`].
        #[doc(hidden)]
        __cause: OpaqueCause,
    },
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Validation(_) => "validation failed",
            Self::Db(_) => "database error",
            Self::Invalid(said)
            | Self::Conflict(said)
            | Self::Forbidden(said)
            | Self::NotFound(said) => said,
            Self::Internal { .. } => nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        })
    }
}

impl fmt::Debug for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (variant, field): (&str, &dyn fmt::Debug) = match self {
            Self::Validation(errors) => ("Validation", SaidFailure::ref_cast(errors)),
            Self::Db(err) => ("Db", err),
            Self::Invalid(said) => ("Invalid", said),
            Self::Conflict(said) => ("Conflict", said),
            Self::Forbidden(said) => ("Forbidden", said),
            Self::NotFound(said) => ("NotFound", said),
            Self::Internal { __cause } => ("Internal", SaidFailure::ref_cast(__cause)),
        };
        f.debug_tuple(variant).field(field).finish()
    }
}

// By hand: thiserror's `#[source]` hands out the field, which quotes what the variant
// refused, and no link at all loses the cause under an `#[error(transparent)]` wrapper.
impl Error for ServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Validation(errors) => Some(SaidFailure::ref_cast(errors)),
            Self::Db(err) => Some(err),
            Self::Internal { __cause } => Some(SaidFailure::ref_cast(__cause)),
            Self::Invalid(_) | Self::Conflict(_) | Self::Forbidden(_) | Self::NotFound(_) => None,
        }
    }
}

impl From<ValidationErrors> for ServiceError {
    fn from(errors: ValidationErrors) -> Self {
        Self::Validation(errors)
    }
}

/// A failure a [`ServiceError`] holds, as its chain hands it out: its `Display` and
/// its `Debug` are what [`error_message`](fn@nest_rs_core::error_message) says of it,
/// where the library's own quote the rejected value or the server's message.
#[derive(RefCast)]
#[repr(transparent)]
struct SaidFailure<T>(T);

impl fmt::Display for SaidFailure<ValidationErrors> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&DecodeError::redact(&self.0.to_string(), None))
    }
}

impl fmt::Display for SaidFailure<OpaqueCause> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sentence = nest_rs_core::error_message(UnderTheConstant::ref_cast(&self.0));
        let below = sentence
            .strip_prefix(nest_rs_core::OPAQUE_CLIENT_MESSAGE)
            .unwrap_or(&sentence);
        f.write_str(below.strip_prefix(": ").unwrap_or(below))
    }
}

/// A cause read beneath the constant `Internal` displays, as its chain is, so its
/// sentence leaves a blank detail out and says one repeating the constant once.
#[derive(Debug, RefCast)]
#[repr(transparent)]
struct UnderTheConstant(OpaqueCause);

impl fmt::Display for UnderTheConstant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(nest_rs_core::OPAQUE_CLIENT_MESSAGE)
    }
}

impl Error for UnderTheConstant {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

impl<T> fmt::Debug for SaidFailure<T>
where
    Self: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.to_string(), f)
    }
}

impl<T: 'static> Error for SaidFailure<T> where Self: fmt::Display {}

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

    /// **500** — an internal failure that is not the database's. The detail
    /// reaches the operator's line only; the wire sees a constant string. It
    /// names what failed, never a value the request carried.
    pub fn internal(detail: impl Into<Cow<'static, str>>) -> Self {
        Self::Internal {
            __cause: OpaqueCause::new(detail, None),
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
            __cause: OpaqueCause::because(detail, source),
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
}

/// A database failure as the class a client branches on: a unique violation is a
/// [`Conflict`](ServiceError::Conflict) (`409`), a row a write found gone is
/// [`NotFound`](ServiceError::NotFound) (`404`), a write the caller's grants do not
/// cover is [`Forbidden`](ServiceError::Forbidden) (`403`), naming the columns as
/// the table does; anything else is [`Db`](ServiceError::Db) — a `503` for a
/// statement Postgres cancelled, a `500` otherwise.
impl From<RepoError> for ServiceError {
    fn from(err: RepoError) -> Self {
        match err {
            RepoError::UniqueViolation(_) => {
                Self::Conflict("the request conflicts with an existing record".into())
            }
            RepoError::NotFound => Self::NotFound("not found".into()),
            RepoError::Forbidden { .. } => Self::Forbidden(err.to_string()),
            _ => Self::Db(err),
        }
    }
}

/// sea-orm's own error, classified as [`RepoError`] classifies it.
impl From<DbErr> for ServiceError {
    fn from(err: DbErr) -> Self {
        Self::from(RepoError::from(err))
    }
}

/// An atomic block that could not keep its promise: a database refusal by its
/// [`RepoError`], anything else an opaque [`Internal`](ServiceError::Internal)
/// whose cause is the `AtomicError`.
impl From<AtomicError> for ServiceError {
    fn from(err: AtomicError) -> Self {
        let err = match err {
            AtomicError::Database(source) => match source.downcast::<RepoError>() {
                Ok(repo) => return Self::from(*repo),
                Err(other) => AtomicError::Database(other),
            },
            other => other,
        };
        Self::internal_because("the atomic block failed", err)
    }
}

/// A database failure as `Repo` hands it out: the class a caller branches on,
/// said by its SQLSTATE and the names the server reported, never by the server's
/// message or its `detail`, which quote the row it refused.
///
/// Every method of [`Repo`](crate::Repo), [`RepoSelect`](crate::RepoSelect) and
/// [`CrudService`](crate::CrudService) answers one, and `?` turns it into the
/// [`ServiceError`] every edge answers. What a service decides on is one `match`
/// away:
///
/// ```
/// use nest_rs_seaorm::{RepoError, ServiceError};
///
/// fn signing_up(created: Result<(), RepoError>) -> Result<(), ServiceError> {
///     match created {
///         Err(RepoError::UniqueViolation(key)) if key.constraint() == Some("users_email_key") => {
///             Err(ServiceError::conflict("that email is registered"))
///         }
///         other => Ok(other?),
///     }
/// }
/// # assert!(signing_up(Ok(())).is_ok());
/// ```
///
/// A variant the server raised carries its [`Diagnostics`]; it can grow within
/// 7.x, so a `match` outside this crate keeps a `_` arm.
#[derive(Clone)]
#[non_exhaustive]
pub enum RepoError {
    /// The row a write addressed is gone, or outside the caller's scope: a
    /// `404`, `NOT_FOUND`.
    NotFound,
    /// The caller may not write the row: a grant's `.fields` withholds each of
    /// `columns`, refused before any statement, or, `columns` empty, the row a
    /// create inserted falls outside the caller's `Create` scope and is rolled
    /// back. A `403`, `FORBIDDEN`.
    #[non_exhaustive]
    Forbidden {
        /// The columns the caller's grants withhold, as the table names them.
        columns: Vec<String>,
    },
    /// A unique key refused the write (SQLSTATE `23505`): a `409`, `CONFLICT`.
    UniqueViolation(Diagnostics),
    /// A foreign key refused the write (SQLSTATE `23503`): a `500`.
    ForeignKeyViolation(Diagnostics),
    /// The transaction lost to a concurrent one: a serialization failure
    /// (SQLSTATE `40001`), or a deadlock Postgres broke by failing it (`40P01`).
    /// Nothing it wrote stands, so running it again may succeed: a `500`.
    SerializationFailure(Diagnostics),
    /// Postgres cancelled the statement (SQLSTATE `57014`), past its
    /// `statement_timeout` most often: a `503`, `UNAVAILABLE`, said at `warn`.
    /// Inside a transaction nothing it wrote stands, so another attempt may
    /// succeed.
    QueryCanceled(Diagnostics),
    /// Any other failure — a refusal no variant names, a connection lost, a
    /// value that would not bind — said by its kind: a `500`.
    #[non_exhaustive]
    Other {
        /// What the failure is, as every reader prints it.
        #[doc(hidden)]
        __said: Arc<str>,
    },
}

impl RepoError {
    /// A failure no variant names, said as `said`.
    fn other(said: impl Into<Arc<str>>) -> Self {
        Self::Other {
            __said: said.into(),
        }
    }

    /// A create the caller's `Create` scope does not cover, refused or rolled
    /// back.
    pub(crate) fn outside_create_scope() -> Self {
        Self::Forbidden {
            columns: Vec::new(),
        }
    }
}

impl fmt::Display for RepoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the row is gone, or outside the caller's scope"),
            Self::Forbidden { columns } if columns.is_empty() => {
                f.write_str("the caller may not write the row")
            }
            Self::Forbidden { columns } => {
                write!(f, "the caller may not write {}", columns.join(", "))
            }
            Self::UniqueViolation(diagnostics)
            | Self::ForeignKeyViolation(diagnostics)
            | Self::SerializationFailure(diagnostics)
            | Self::QueryCanceled(diagnostics) => fmt::Display::fmt(diagnostics, f),
            Self::Other { __said } => f.write_str(__said),
        }
    }
}

impl fmt::Debug for RepoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (variant, diagnostics) = match self {
            Self::NotFound => return f.write_str("NotFound"),
            Self::Forbidden { columns } => {
                return f
                    .debug_struct("Forbidden")
                    .field("columns", columns)
                    .finish();
            }
            Self::Other { __said } => return f.debug_tuple("Other").field(__said).finish(),
            Self::UniqueViolation(diagnostics) => ("UniqueViolation", diagnostics),
            Self::ForeignKeyViolation(diagnostics) => ("ForeignKeyViolation", diagnostics),
            Self::SerializationFailure(diagnostics) => ("SerializationFailure", diagnostics),
            Self::QueryCanceled(diagnostics) => ("QueryCanceled", diagnostics),
        };
        f.debug_tuple(variant).field(diagnostics).finish()
    }
}

/// Everything a `RepoError` knows is in its sentence: no link below it holds the
/// server's own words.
impl Error for RepoError {}

/// sea-orm's error read into its class: a server's refusal by its SQLSTATE, a
/// write `Repo` found no row for, or one a field grant refused, by what it
/// means, anything else said by its kind.
impl From<DbErr> for RepoError {
    fn from(err: DbErr) -> Self {
        match &err {
            DbErr::RecordNotUpdated | DbErr::RecordNotFound(_) => return Self::NotFound,
            DbErr::AccessDenied { .. } => {
                return Self::Forbidden {
                    columns: refused_columns(&err)
                        .map(|columns| columns.split(", ").map(str::to_owned).collect())
                        .unwrap_or_default(),
                };
            }
            _ => {}
        }
        let Some(server) = server_error(&err) else {
            return Self::other(said_db(&err));
        };
        let diagnostics = Diagnostics::of(server);
        match server.kind() {
            ErrorKind::UniqueViolation => Self::UniqueViolation(diagnostics),
            ErrorKind::ForeignKeyViolation => Self::ForeignKeyViolation(diagnostics),
            _ => match diagnostics.sqlstate() {
                SERIALIZATION_FAILURE | DEADLOCK_DETECTED => {
                    Self::SerializationFailure(diagnostics)
                }
                CANCELLED => Self::QueryCanceled(diagnostics),
                _ => Self::other(said_db(&err)),
            },
        }
    }
}

/// An atomic block whose body speaks `RepoError`: the database's refusal of its
/// settling by its class, anything else said as the block's failure.
impl From<AtomicError> for RepoError {
    fn from(err: AtomicError) -> Self {
        let err = match err {
            AtomicError::Database(source) => match source.downcast::<Self>() {
                Ok(repo) => return *repo,
                Err(other) => AtomicError::Database(other),
            },
            other => other,
        };
        Self::other(nest_rs_core::error_message(&err))
    }
}

/// What a database server reported of a failure beside its message: its
/// SQLSTATE and the names it gave — the constraint, the table, the column. A
/// name is the schema's, never a value a row holds.
#[derive(Clone, PartialEq, Eq)]
pub struct Diagnostics(Box<Reported>);

#[derive(Clone, PartialEq, Eq)]
struct Reported {
    sqlstate: Box<str>,
    constraint: Option<Box<str>>,
    table: Option<Box<str>>,
    column: Option<Box<str>>,
}

impl Diagnostics {
    /// What `server` reported, its message and `detail` left behind.
    fn of(server: &(dyn DatabaseError + 'static)) -> Self {
        let column = server
            .try_downcast_ref::<PgDatabaseError>()
            .and_then(PgDatabaseError::column);
        Self(Box::new(Reported {
            sqlstate: server.code().unwrap_or_default().into(),
            constraint: server.constraint().map(Into::into),
            table: server.table().map(Into::into),
            column: column.map(Into::into),
        }))
    }

    /// The five-character SQLSTATE the server answered (`23505`).
    pub fn sqlstate(&self) -> &str {
        &self.0.sqlstate
    }

    /// The constraint the server named: a unique key's, a foreign key's.
    pub fn constraint(&self) -> Option<&str> {
        self.0.constraint.as_deref()
    }

    /// The table the server named.
    pub fn table(&self) -> Option<&str> {
        self.0.table.as_deref()
    }

    /// The column the server named.
    pub fn column(&self) -> Option<&str> {
        self.0.column.as_deref()
    }
}

/// `database error <SQLSTATE>`, then the constraint, table and column the
/// server named: schema an operator searches for, where its message quotes the
/// row.
impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("database error")?;
        if !self.sqlstate().is_empty() {
            write!(f, " {}", self.sqlstate())?;
        }
        let mut separator = " on";
        for (what, name) in [
            ("constraint", self.constraint()),
            ("table", self.table()),
            ("column", self.column()),
        ] {
            if let Some(name) = name {
                write!(f, "{separator} {what} {name:?}")?;
                separator = ",";
            }
        }
        Ok(())
    }
}

impl fmt::Debug for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Diagnostics")
            .field("sqlstate", &self.sqlstate())
            .field("constraint", &self.constraint())
            .field("table", &self.table())
            .field("column", &self.column())
            .finish()
    }
}

/// The refusal of a write setting what a field grant withholds, as the sea-orm
/// path a write takes carries it: a `DbErr` saying what `GRANT UPDATE (title) ON
/// posts` would, which `From<DbErr>` reads back into [`RepoError::Forbidden`].
pub(crate) fn write_refused(statement: &str, table: &str, columns: &[&str]) -> DbErr {
    DbErr::AccessDenied {
        permission: statement.to_owned(),
        resource: format!("{table}({})", columns.join(", ")),
    }
}

/// The columns [`write_refused`] named, as it joined them.
fn refused_columns(err: &DbErr) -> Option<&str> {
    let DbErr::AccessDenied { resource, .. } = err else {
        return None;
    };
    resource.split_once('(')?.1.strip_suffix(')')
}

/// SQLSTATE `query_canceled`: a statement Postgres cancelled, at its
/// `statement_timeout` or by an operator.
pub(crate) const CANCELLED: &str = "57014";

/// SQLSTATE `serialization_failure`.
pub(crate) const SERIALIZATION_FAILURE: &str = "40001";

/// SQLSTATE `deadlock_detected`, Postgres's own.
pub(crate) const DEADLOCK_DETECTED: &str = "40P01";

/// Whether Postgres cancelled the statement `err` reports.
pub(crate) fn is_cancelled(err: &DbErr) -> bool {
    server_error(err).is_some_and(|server| server.code().as_deref() == Some(CANCELLED))
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
        DbErr::RbacError(_) => "RBAC error".to_owned(),
        // Each names a table, a column, a pool's state, a count or a sentence
        // the framework or a migration wrote: never a value a client sent.
        DbErr::ConnectionAcquire(_)
        | DbErr::ConvertFromU64(_)
        | DbErr::UnpackInsertId
        | DbErr::UpdateGetPrimaryKey
        | DbErr::RecordNotFound(_)
        | DbErr::AttrNotSet(_)
        | DbErr::Custom(_)
        | DbErr::Migration(_)
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

/// What `server` reported, as its [`Diagnostics`] say it.
fn said_server(server: &(dyn DatabaseError + 'static)) -> String {
    Diagnostics::of(server).to_string()
}

/// What every edge answers for a `ServiceError`, HTTP included: a business
/// variant's problem, its sentence as the detail and a validation failure's
/// field map as `errors`; a cancelled statement's `503` and its wait; `None`
/// for any other `Db` and `Internal`, which the edge answers
/// opaquely, logging the cause.
impl nest_rs_core::ToProblem for ServiceError {
    fn to_problem(&self) -> Option<nest_rs_core::Problem> {
        use nest_rs_core::{Problem, code};

        let problem = match self {
            Self::Validation(_) => Problem::new(400, code::INVALID_ARGUMENT),
            Self::Invalid(_) => Problem::new(422, code::INVALID_ARGUMENT),
            Self::Conflict(_) => Problem::new(409, code::CONFLICT),
            Self::Forbidden(_) => Problem::new(403, code::FORBIDDEN),
            Self::NotFound(_) => Problem::new(404, code::NOT_FOUND),
            Self::Db(RepoError::QueryCanceled(_)) => unavailable(),
            Self::Db(_) | Self::Internal { .. } => return None,
        };
        let problem = problem.with_detail(self.to_string());
        Some(with_field_errors(problem, self.field_errors()))
    }
}

/// `problem`, with `fields` as its `errors` when there are any.
fn with_field_errors(
    problem: nest_rs_core::Problem,
    fields: Option<serde_json::Value>,
) -> nest_rs_core::Problem {
    match fields {
        Some(fields) => problem.with_errors(fields),
        None => problem,
    }
}

/// What a statement Postgres cancelled answers on every edge: `503`,
/// `UNAVAILABLE`, and the wait the HTTP edge's own deadline names — the budget
/// that ran out, here the statement bound of the pool the unit ran on, in whole
/// seconds rounded up. Outside a unit, or on a pool with no bound, no wait is
/// invented.
fn unavailable() -> nest_rs_core::Problem {
    use nest_rs_core::__private::retry_after_secs;
    use nest_rs_core::{Problem, code};

    let problem = Problem::new(503, code::UNAVAILABLE);
    let bound = crate::executor::current_executor()
        .as_ref()
        .map(crate::executor::SeaOrmExecutor::pool)
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
/// violation is a `409` — naming, for a create or an update, the fields of
/// the input `written` the violated key holds — a write the caller's grants do
/// not cover a `403`, naming the fields of `written` that set a column they
/// withhold, a row gone before the write a `404`, a statement Postgres
/// cancelled a `503` filed at `warn`; any other failure a `500`, its error filed
/// once at `error`.
#[cfg(any(feature = "http", feature = "graphql"))]
fn crud_problem(err: &RepoError, written: Option<Written>) -> nest_rs_core::Problem {
    use nest_rs_core::{Problem, code};

    match err {
        RepoError::UniqueViolation(diagnostics) => with_field_errors(
            Problem::new(409, code::CONFLICT),
            written.and_then(|written| {
                taken_fields(diagnostics, written, &*crate::unique_key::current()?)
            }),
        ),
        RepoError::NotFound => Problem::new(404, code::NOT_FOUND),
        RepoError::Forbidden { columns } => with_field_errors(
            Problem::new(403, code::FORBIDDEN),
            written.and_then(|written| withheld_fields(columns, written)),
        ),
        RepoError::QueryCanceled(_) => {
            tracing::warn!(
                target: crate::target::ORM,
                kind = "db",
                error = %nest_rs_core::error_message(err),
                "crud operation unavailable",
            );
            unavailable()
        }
        _ => {
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

/// The input a `#[crud]` create or update sent: the table its entity writes,
/// and the field it carries a column in.
#[cfg(any(feature = "http", feature = "graphql"))]
#[derive(Clone, Copy)]
struct Written {
    table: &'static str,
    field_for: fn(&str) -> Option<&'static str>,
}

#[cfg(any(feature = "http", feature = "graphql"))]
impl Written {
    fn on<E: sea_orm::EntityTrait>(field_for: fn(&str) -> Option<&'static str>) -> Self {
        Self {
            table: sea_orm::EntityName::table_name(&E::default()),
            field_for,
        }
    }
}

/// The code a field's entry carries when a unique key holds its value: the
/// key's kind, as a validation failure carries its rule's name.
#[cfg(any(feature = "http", feature = "graphql"))]
const UNIQUE: &str = "unique";

/// The fields of `written`'s input the unique key `violated` names holds, each
/// `[{"code": "unique"}]` — the shape [`ServiceError::field_errors`] gives a
/// validation failure — and never the value. `None` when the violation is on
/// another table and none of its partitions, on a key `keys` does not know, or
/// on columns the input does not carry.
#[cfg(any(feature = "http", feature = "graphql"))]
fn taken_fields(
    violated: &Diagnostics,
    written: Written,
    keys: &crate::unique_key::UniqueKeys,
) -> Option<serde_json::Value> {
    let fields: serde_json::Map<String, serde_json::Value> = keys
        .columns(written.table, violated.table()?, violated.constraint()?)?
        .iter()
        .filter_map(|column| (written.field_for)(column))
        .map(|field| (field.to_owned(), serde_json::json!([{ "code": UNIQUE }])))
        .collect();
    (!fields.is_empty()).then_some(serde_json::Value::Object(fields))
}

/// The code a field's entry carries when a field grant withholds its column: the
/// class it answers, read beside the field.
#[cfg(any(feature = "http", feature = "graphql"))]
const WITHHELD: &str = "forbidden";

/// The fields of `written`'s input setting one of the `columns` a grant
/// withholds, each `[{"code": "forbidden"}]`; `None` when the input carries none
/// of them, a service having set what it refused.
#[cfg(any(feature = "http", feature = "graphql"))]
fn withheld_fields(columns: &[String], written: Written) -> Option<serde_json::Value> {
    let fields: serde_json::Map<String, serde_json::Value> = columns
        .iter()
        .filter_map(|column| (written.field_for)(column))
        .map(|field| (field.to_owned(), serde_json::json!([{ "code": WITHHELD }])))
        .collect();
    (!fields.is_empty()).then_some(serde_json::Value::Object(fields))
}

#[cfg(feature = "http")]
pub use http::{crud_error as http_crud_error, crud_write_error as http_crud_write_error};

#[cfg(feature = "graphql")]
pub use graphql::{crud_error as graphql_crud_error, crud_write_error as graphql_crud_write_error};

#[cfg(feature = "graphql")]
mod graphql {
    use nest_rs_graphql::async_graphql::Error;

    use super::RepoError;

    /// A GraphQL `#[crud]` operation's failure as the class a client branches
    /// on: the HTTP twin's statuses, as `CONFLICT`, `FORBIDDEN`, `NOT_FOUND` or
    /// the opaque `INTERNAL`. Called by the `#[crud]` expansion.
    pub fn crud_error(err: RepoError) -> Error {
        nest_rs_graphql::problem_error(&super::crud_problem(&err, None))
    }

    /// [`crud_error`] for a create or an update on `E` whose input names a
    /// column through `field_for`: a unique conflict, or a write a grant's `.fields`
    /// refuses, names the input's fields under `extensions.errors`. Called by the
    /// `#[crud]` expansion.
    pub fn crud_write_error<E: sea_orm::EntityTrait>(
        field_for: fn(&str) -> Option<&'static str>,
    ) -> impl FnOnce(RepoError) -> Error {
        move |err| {
            let written = super::Written::on::<E>(field_for);
            nest_rs_graphql::problem_error(&super::crud_problem(&err, Some(written)))
        }
    }

    #[cfg(test)]
    mod tests {
        use nest_rs_core::{OPAQUE_CLIENT_MESSAGE, code};
        use nest_rs_graphql::CODE_EXTENSION;

        use super::*;

        fn answered(err: impl Into<RepoError>) -> (String, serde_json::Value) {
            let error = crud_error(err.into());
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
            let (said, code) = answered(sea_orm::DbErr::RecordNotUpdated);
            assert_eq!(
                (said.as_str(), code),
                ("not found", code::NOT_FOUND.as_str().into())
            );
            let (said, code) = answered(RepoError::outside_create_scope());
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
        async fn a_crud_write_conflict_names_the_inputs_field_under_its_extensions() {
            use crate::error::tests::{
                taken_email, under_a_binding_holding, user, users_input, users_keys,
            };

            let error = under_a_binding_holding(users_keys(), async {
                crud_write_error::<user::Entity>(users_input)(taken_email().into())
            })
            .await;
            assert_eq!(error.message, "conflict");
            let extensions = serde_json::to_value(&error.extensions).expect("extensions serialize");
            assert_eq!(extensions[CODE_EXTENSION], code::CONFLICT.as_str());
            assert_eq!(
                extensions[nest_rs_graphql::FIELD_ERRORS_EXTENSION],
                serde_json::json!({ "email": [{ "code": "unique" }] }),
                "{extensions}",
            );
        }

        #[tokio::test]
        async fn a_cancelled_statement_tells_a_graphql_client_its_wait() {
            use crate::error::tests::{
                STATEMENT_BOUND, cancelled_statement, on_a_pool_bounding_statements,
            };

            let (crud, service) = on_a_pool_bounding_statements(STATEMENT_BOUND, || {
                (
                    crud_error(cancelled_statement().into()),
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
        fn a_write_a_field_grant_refuses_names_the_inputs_field_under_its_extensions() {
            use crate::error::tests::{user, users_input};

            let refused = super::super::write_refused("insert", "users", &["handle", "id"]);
            let error = crud_write_error::<user::Entity>(users_input)(refused.into());

            assert_eq!(error.message, "forbidden");
            let extensions = serde_json::to_value(&error.extensions).expect("extensions serialize");
            assert_eq!(extensions[CODE_EXTENSION], code::FORBIDDEN.as_str());
            assert_eq!(
                extensions[nest_rs_graphql::FIELD_ERRORS_EXTENSION],
                serde_json::json!({ "slug": [{ "code": "forbidden" }] }),
                "the field as the client spells it, and no column it does not carry: {extensions}",
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

        /// The conversion answers opaquely and keeps the chain for the edge,
        /// which files it once the answer is final (the integration suite's
        /// `a_service_error_from_the_database_answers_its_class_and_never_its_value`):
        /// the detail, then the cause.
        #[test]
        fn an_internal_failure_answers_opaquely_and_keeps_its_detail_for_the_operator() {
            let logs = nest_rs_testing::LogCapture::install();

            let failure = crate::ServiceError::internal_because(
                "could not sign the export",
                std::io::Error::other("the key store refused the connection"),
            );
            assert_eq!(
                nest_rs_core::error_message(&failure),
                "internal error: could not sign the export: the key store refused the connection",
            );
            let error = Error::from(failure);
            assert_eq!(error.message, OPAQUE_CLIENT_MESSAGE);
            logs.expect_none(nest_rs_graphql::TARGET, "graphql operation failed");
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
    use nest_rs_http::HttpError;

    /// A `#[crud]` operation's failure as the problem document of its class:
    /// `409` for a unique violation, `403` for a create its scope re-check
    /// rolled back or a write a grant's `.fields` refused, `404` for a row gone
    /// before the write, `503` for a statement Postgres cancelled, else a `500`
    /// whose error is filed at `error` — each with the `code` member the other
    /// edges answer. Called by the `#[crud]` expansion.
    pub fn crud_error(err: super::RepoError) -> HttpError {
        HttpError::from(super::crud_problem(&err, None))
    }

    /// [`crud_error`] for a create or an update on `E` whose input names a
    /// column through `field_for`: a unique conflict, or a write a grant's `.fields`
    /// refuses, names the input's fields under `errors`. Called by the `#[crud]`
    /// expansion.
    pub fn crud_write_error<E: sea_orm::EntityTrait>(
        field_for: fn(&str) -> Option<&'static str>,
    ) -> impl FnOnce(super::RepoError) -> HttpError {
        move |err| {
            let written = super::Written::on::<E>(field_for);
            HttpError::from(super::crud_problem(&err, Some(written)))
        }
    }

    #[cfg(test)]
    mod tests {
        use nest_rs_core::code;
        use nest_rs_http::{Response, StatusCode};

        use super::super::{RepoError, ServiceError};
        use super::*;

        /// A value breaking its type's rules answers what the edge's own check
        /// of it answers, whoever checks it.
        #[test]
        fn validation_is_400_as_the_edges_check_is() {
            let err = ServiceError::Validation(validator::ValidationErrors::new());
            assert_eq!(HttpError::from(err).status(), StatusCode::BAD_REQUEST);
            let edge = nest_rs_http::pipe_error(&nest_rs_pipes::PipeError::from(
                validator::ValidationErrors::new(),
            ));
            assert_eq!(edge.status(), StatusCode::BAD_REQUEST);
        }

        #[test]
        fn db_is_500() {
            let err = ServiceError::Db(RepoError::from(sea_orm::DbErr::Custom("boom".into())));
            assert_eq!(
                HttpError::from(err).status(),
                StatusCode::INTERNAL_SERVER_ERROR
            );
        }

        /// Rendered for the HTTP edge, which files what a `5xx` withholds
        /// once the answer is final, no variant files anything of its own.
        #[test]
        fn a_variant_rendered_for_the_edge_files_nothing_of_its_own() {
            let logs = nest_rs_testing::LogCapture::install();
            for error in [
                ServiceError::Db(RepoError::from(crate::error::tests::bad_uuid())),
                ServiceError::Db(RepoError::from(crate::error::tests::cancelled_statement())),
                ServiceError::internal("could not sign the export"),
                ServiceError::not_found("no such widget"),
            ] {
                let _ = nest_rs_http::ResponseError::as_response(&error);
            }
            assert!(logs.events().is_empty(), "{:#?}", logs.events());
        }

        #[test]
        fn a_crud_500_emits_the_cause_and_the_mapped_statuses_do_not() {
            let logs = nest_rs_testing::LogCapture::install();

            let mapped = crud_error(super::super::RepoError::outside_create_scope());
            assert_eq!(mapped.status(), StatusCode::FORBIDDEN);
            let taken = crud_error(crate::error::tests::taken_email().into());
            assert_eq!(taken.status(), StatusCode::CONFLICT);
            let vanished = crud_error(sea_orm::DbErr::RecordNotUpdated.into());
            assert_eq!(vanished.status(), StatusCode::NOT_FOUND);

            let unexpected = crud_error(sea_orm::DbErr::Custom("deadlock detected".into()).into());
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
            let unavailable = crud_error(crate::error::tests::cancelled_statement().into());
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
            assert_eq!(body["code"], nest_rs_core::code::UNAVAILABLE.as_str());
        }

        #[test]
        fn a_crud_500_files_the_database_error_without_the_value_it_quoted() {
            let logs = nest_rs_testing::LogCapture::install();
            let failed = crud_error(crate::error::tests::bad_uuid().into());
            assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);

            let event = logs.expect_one(crate::target::ORM, "crud operation failed");
            let said = event.field("error").unwrap_or_default();
            assert!(said.contains("22P02"), "{said}");
            assert!(!said.contains("secret-123"), "{said}");
        }

        #[test]
        fn business_variants_map_to_their_4xx() {
            for (error, status) in [
                (ServiceError::invalid("x"), StatusCode::UNPROCESSABLE_ENTITY),
                (ServiceError::conflict("x"), StatusCode::CONFLICT),
                (ServiceError::forbidden("x"), StatusCode::FORBIDDEN),
                (ServiceError::not_found("x"), StatusCode::NOT_FOUND),
                (
                    ServiceError::internal("x"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                ),
            ] {
                assert_eq!(HttpError::from(error).status(), status);
            }
        }

        async fn body_json(err: ServiceError) -> (StatusCode, Option<String>, serde_json::Value) {
            body_of(HttpError::from(err).into_response()).await
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

        /// A `#[crud]` write a unique key refuses names, under `errors`, the
        /// input's fields the key holds, read off the binding the request's
        /// unit was opened by; outside one it names nothing, and no value,
        /// index or table reaches the body.
        #[tokio::test]
        async fn a_crud_write_conflict_names_the_inputs_field_under_errors() {
            use crate::error::tests::{
                taken_email, under_a_binding_holding, user, users_input, users_keys,
            };

            let logs = nest_rs_testing::LogCapture::install();
            let (status, ct, body) = under_a_binding_holding(users_keys(), async {
                body_json_of(crud_write_error::<user::Entity>(users_input)(
                    taken_email().into(),
                ))
                .await
            })
            .await;
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(ct.as_deref(), Some("application/problem+json"));
            assert_eq!(body["code"], code::CONFLICT.as_str(), "{body}");
            assert_eq!(
                body["errors"],
                serde_json::json!({ "email": [{ "code": "unique" }] }),
                "{body}",
            );
            for withheld in ["ada@example.com", "users_email_key", "users", "23505"] {
                assert!(!body.to_string().contains(withheld), "{withheld} in {body}");
            }
            assert!(
                logs.events()
                    .iter()
                    .all(|event| event.level != "warn" && event.level != "error"),
                "a conflict is no incident: {:#?}",
                logs.events(),
            );

            let (_, _, outside) = body_json_of(crud_write_error::<user::Entity>(users_input)(
                taken_email().into(),
            ))
            .await;
            assert!(outside.get("errors").is_none(), "{outside}");
            let (_, _, read) = under_a_binding_holding(users_keys(), async {
                body_json_of(crud_error(taken_email().into())).await
            })
            .await;
            assert!(
                read.get("errors").is_none(),
                "a read sent no input to name: {read}"
            );
        }

        /// The field as the client spells it, under `errors`; the column a service
        /// set by itself names nothing, and neither reaches the body as the table
        /// spells it.
        #[tokio::test]
        async fn a_write_a_field_grant_refuses_names_the_inputs_field_under_errors() {
            use crate::error::tests::{user, users_input};

            let refused = super::super::write_refused("update", "users", &["handle", "id"]);
            let (status, ct, body) = body_json_of(crud_write_error::<user::Entity>(users_input)(
                refused.into(),
            ))
            .await;

            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(ct.as_deref(), Some("application/problem+json"));
            assert_eq!(body["code"], code::FORBIDDEN.as_str(), "{body}");
            assert_eq!(
                body["errors"],
                serde_json::json!({ "slug": [{ "code": "forbidden" }] }),
                "{body}",
            );
            for withheld in ["handle", "users"] {
                assert!(!body.to_string().contains(withheld), "{withheld} in {body}");
            }

            let only_the_services = super::super::write_refused("update", "users", &["id"]);
            let (status, _, body) = body_json_of(crud_write_error::<user::Entity>(users_input)(
                only_the_services.into(),
            ))
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert!(body.get("errors").is_none(), "{body}");
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
            let (status, ct, body) = body_json(ServiceError::Db(
                sea_orm::DbErr::Custom("SELECT password_hash FROM user".into()).into(),
            ))
            .await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(ct.as_deref(), Some("application/problem+json"));
            assert_eq!(body["status"], 500);
            assert_eq!(body["code"], code::INTERNAL.as_str(), "{body}");
            assert!(body.get("detail").is_none(), "a 5xx says no detail: {body}");
            let (_, _, crud) = body_json_of(crud_error(
                sea_orm::DbErr::Custom("SELECT password_hash FROM user".into()).into(),
            ))
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
                    nest_rs_http::ResponseError::as_response(&ServiceError::from(
                        cancelled_statement(),
                    )),
                    crud_error(cancelled_statement().into()).into_response(),
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
                assert_eq!(body["code"], nest_rs_core::code::UNAVAILABLE.as_str());
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
                    nest_rs_core::code::CONFLICT,
                ),
                (
                    ServiceError::NotFound("post".into()),
                    nest_rs_core::code::NOT_FOUND,
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
            assert_eq!(status, StatusCode::BAD_REQUEST);
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
    /// the SQLSTATE its kind is read off, as sqlx reads a Postgres one's, and
    /// the names Postgres reports beside it.
    #[derive(Debug)]
    struct ServerRefusal {
        code: &'static str,
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
            match self.code {
                "23505" => ErrorKind::UniqueViolation,
                "23503" => ErrorKind::ForeignKeyViolation,
                _ => ErrorKind::Other,
            }
        }
    }

    fn refused(refusal: ServerRefusal) -> DbErr {
        DbErr::Exec(RuntimeErr::SqlxError(Arc::new(SqlxError::Database(
            Box::new(refusal),
        ))))
    }

    pub(crate) fn taken_email() -> DbErr {
        refused(ServerRefusal {
            code: "23505",
            constraint: Some("users_email_key"),
            table: Some("users"),
            message: "duplicate key value violates unique constraint: (email)=(ada@example.com)",
        })
    }

    /// A unique violation on `index` of `table`, as Postgres reports one:
    /// its message quoting the row.
    #[cfg(any(feature = "http", feature = "graphql"))]
    fn unique_violation(index: &'static str, table: &'static str) -> DbErr {
        refused(ServerRefusal {
            code: "23505",
            constraint: Some(index),
            table: Some(table),
            message: "duplicate key value violates unique constraint: (slug)=(secret-slug)",
        })
    }

    /// The entity the tests' `users` input writes.
    #[cfg(any(feature = "http", feature = "graphql"))]
    pub(crate) mod user {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "users")]
        pub(crate) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub email: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(crate) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    /// The fields the tests' `users` input carries, by column.
    #[cfg(any(feature = "http", feature = "graphql"))]
    pub(crate) fn users_input(column: &str) -> Option<&'static str> {
        match column {
            "email" => Some("email"),
            "handle" => Some("slug"),
            _ => None,
        }
    }

    /// The unique keys of the tests' `users` table.
    #[cfg(any(feature = "http", feature = "graphql"))]
    pub(crate) fn users_keys() -> crate::unique_key::UniqueKeys {
        crate::unique_key::tests::keys(&[
            ("users", "users_email_key", &["email"]),
            ("users", "users_org_handle_live", &["org_id", "handle"]),
            ("users", "users_org_key", &["org_id"]),
        ])
    }

    /// `body` inside a unit the binding opened, the binding holding `keys`, as
    /// a `#[crud]` write's error is rendered at its edge. Nothing is dialled.
    #[cfg(any(feature = "http", feature = "graphql"))]
    pub(crate) async fn under_a_binding_holding<T: Send>(
        keys: crate::unique_key::UniqueKeys,
        body: impl Future<Output = T> + Send,
    ) -> T {
        use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec};

        let binding: Arc<dyn UnitOfWork> = Arc::new(
            crate::SeaOrmUnitOfWork::new(
                Arc::new(sea_orm::DatabaseConnection::default()),
                &crate::SeaOrmConfig::default(),
            )
            .reading(keys),
        );
        nest_rs_database::run_unit(
            Some(&binding),
            UnitSpec::new(
                nest_rs_core::unit::HOOK,
                ExecutorScope::Request,
                TransactionPolicy::Pool,
            ),
            body,
            |_| true,
            |why| panic!("a pool unit is always opened: {why}"),
        )
        .await
    }

    #[cfg(any(feature = "http", feature = "graphql"))]
    #[test]
    fn a_unique_conflict_names_the_fields_of_the_input_its_key_holds() {
        let written = Written {
            table: "users",
            field_for: users_input,
        };
        let keys = users_keys();
        let unique = serde_json::json!([{ "code": "unique" }]);

        assert_eq!(
            taken_fields(&violated(taken_email()), written, &keys),
            Some(serde_json::json!({ "email": unique })),
        );
        assert_eq!(
            taken_fields(
                &violated(unique_violation("users_org_handle_live", "users")),
                written,
                &keys
            ),
            Some(serde_json::json!({ "slug": unique })),
            "a key's column the input does not carry is no field the client sent",
        );
        for (violation, why) in [
            (
                unique_violation("users_org_key", "users"),
                "a key the input carries no column of",
            ),
            (
                unique_violation("users_handle_lower", "users"),
                "a key the binding did not read",
            ),
            (
                unique_violation("users_email_key", "audit_log"),
                "another table's key, a hook's insert",
            ),
        ] {
            assert_eq!(
                taken_fields(&violated(violation), written, &keys),
                None,
                "{why}"
            );
        }
    }

    /// What the server reported of the unique violation `err` is.
    #[cfg(any(feature = "http", feature = "graphql"))]
    fn violated(err: DbErr) -> Diagnostics {
        match RepoError::from(err) {
            RepoError::UniqueViolation(diagnostics) => diagnostics,
            other => panic!("a unique violation: {other:?}"),
        }
    }

    /// The database failure a `ServiceError` holds for a `match` prints, by every
    /// reader, what `error_message` says of it: no value the server quoted.
    #[test]
    fn the_database_failure_a_service_error_holds_prints_no_value_by_any_reader() {
        let ServiceError::Db(held) = ServiceError::from(bad_uuid()) else {
            panic!("an unclassified refusal stays a database error");
        };
        let printed = nest_rs_testing::printed(&held);
        assert!(printed.contains("22P02"), "{printed}");
        assert!(!printed.contains("secret-123"), "{printed}");
    }

    #[test]
    fn an_atomic_block_the_database_refused_is_classified_like_its_failure() {
        let conflict = ServiceError::from(AtomicError::Database(Box::new(RepoError::from(
            taken_email(),
        ))));
        assert!(
            matches!(conflict, ServiceError::Conflict(_)),
            "{conflict:?}"
        );

        let opaque =
            ServiceError::from(AtomicError::Database(Box::new(RepoError::from(bad_uuid()))));
        let ServiceError::Db(err) = opaque else {
            panic!("an unclassified refusal stays a database error: {opaque:?}");
        };
        assert!(!error_message(&err).contains("secret-123"));
    }

    #[test]
    fn every_other_atomic_failure_is_internal_and_says_it_as_its_cause() {
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
            assert_eq!(
                error_message(&internal),
                format!("internal error: the atomic block failed: {said}")
            );
        }
    }

    #[test]
    fn an_atomic_block_over_a_repo_body_answers_its_class() {
        let refused = RepoError::from(AtomicError::Database(Box::new(RepoError::NotFound)));
        assert!(matches!(refused, RepoError::NotFound), "{refused:?}");

        let poisoned = RepoError::from(AtomicError::Poisoned { retryable: true });
        assert!(
            matches!(&poisoned, RepoError::Other { .. })
                && poisoned.to_string().contains("inside the atomic block"),
            "{poisoned:?}"
        );
    }

    pub(super) fn bad_uuid() -> DbErr {
        refused(ServerRefusal {
            code: "22P02",
            constraint: None,
            table: None,
            message: "invalid input syntax for type uuid: \"secret-123\"",
        })
    }

    /// Postgres cancelling a statement past its `statement_timeout`.
    pub(crate) fn cancelled_statement() -> DbErr {
        refused(ServerRefusal {
            code: "57014",
            constraint: None,
            table: None,
            message: "canceling statement due to statement timeout",
        })
    }

    #[test]
    fn a_statement_cancelled_at_its_bound_is_the_server_unable_to_serve_in_time() {
        assert!(
            matches!(RepoError::from(cancelled_statement()), RepoError::QueryCanceled(diagnostics) if diagnostics.sqlstate() == "57014"),
        );
        let error = ServiceError::from(cancelled_statement());
        assert!(matches!(error, ServiceError::Db(_)), "{error:?}");
        let problem = nest_rs_core::ToProblem::to_problem(&error)
            .expect("an unavailable answer, not an opaque one");
        assert_eq!(
            (problem.status(), problem.code()),
            (503, nest_rs_core::code::UNAVAILABLE)
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
        nest_rs_database::__private::with_executor(
            crate::executor::SeaOrmExecutor::Pool(pool).into(),
            async move { read() },
        )
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
            (503, nest_rs_core::code::UNAVAILABLE, Some(3)),
        );
        let outside =
            nest_rs_core::ToProblem::to_problem(&ServiceError::from(cancelled_statement()))
                .expect("an unavailable answer");
        assert_eq!(outside.retry_after(), None);
    }

    #[tokio::test]
    async fn a_cancelled_statement_tells_a_websocket_client_its_wait() {
        let reply = on_a_pool_bounding_statements(STATEMENT_BOUND, || {
            nest_rs_ws::__private::ErrorReport::new(ServiceError::from(cancelled_statement()))
                .into_failure()
                .into_reply("sync")
        })
        .await;
        let nest_rs_ws::WsReply::Error(frame) = reply else {
            panic!("a handler's error answers an error frame");
        };
        assert_eq!(
            serde_json::to_value(&frame).expect("a frame"),
            serde_json::json!({
                "error": nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE,
                "code": "UNAVAILABLE",
                "retryAfterSeconds": 3,
            }),
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
        let classed = |err: DbErr| format!("{:?}", RepoError::from(err));
        assert_eq!(
            classed(taken_email()),
            "UniqueViolation(Diagnostics { sqlstate: \"23505\", constraint: \
             Some(\"users_email_key\"), table: Some(\"users\"), column: None })"
        );
        assert_eq!(classed(DbErr::RecordNotUpdated), "NotFound");
        assert_eq!(classed(DbErr::RecordNotFound("widget".into())), "NotFound");
        // sea-orm raises it for an `ON CONFLICT DO NOTHING` that inserted nothing too.
        assert_eq!(
            classed(DbErr::RecordNotInserted),
            "Other(\"None of the records are inserted\")"
        );
        assert_eq!(
            classed(DbErr::Custom("boom".into())),
            "Other(\"Custom Error: boom\")"
        );
        assert_eq!(
            classed(bad_uuid()),
            "Other(\"Execution Error: database error 22P02\")"
        );
        for (code, class) in [
            ("23503", "ForeignKeyViolation"),
            ("40001", "SerializationFailure"),
            ("40P01", "SerializationFailure"),
            ("57014", "QueryCanceled"),
        ] {
            let said = classed(refused(ServerRefusal {
                code,
                constraint: Some("posts_org_id_fkey"),
                table: Some("posts"),
                message: "a message quoting (org_id)=(secret-123)",
            }));
            assert!(said.starts_with(class), "{code}: {said}");
            assert!(
                said.contains(code) && !said.contains("secret-123"),
                "{said}"
            );
        }
    }

    /// A refused write crosses `Repo` as the columns a `GRANT` would name and
    /// answers `403` on every edge a service reaches.
    #[test]
    fn a_write_a_field_grant_refuses_is_forbidden_naming_its_columns() {
        let refused = write_refused("update", "posts", &["title", "status"]);

        assert!(
            matches!(RepoError::from(refused.clone()), RepoError::Forbidden { columns } if columns == ["title", "status"]),
        );
        assert_eq!(
            error_message(&refused),
            "Access denied: cannot perform `update` on `posts(title, status)`",
        );
        let error = ServiceError::from(refused);
        assert!(
            matches!(
                &error,
                ServiceError::Forbidden(said) if said == "the caller may not write title, status"
            ),
            "{error:?}",
        );
        let problem = nest_rs_core::ToProblem::to_problem(&error).expect("a client's class");
        assert_eq!(problem.status(), 403);
        assert_eq!(problem.code(), nest_rs_core::code::FORBIDDEN);
    }

    /// No other `AccessDenied` exists but one sea-orm's own access control raises,
    /// which names a table alone: refused all the same, with no column to name.
    #[test]
    fn an_access_denial_with_no_column_list_is_forbidden_all_the_same() {
        let refused = DbErr::AccessDenied {
            permission: "update".to_owned(),
            resource: "posts".to_owned(),
        };

        assert!(
            matches!(ServiceError::from(refused), ServiceError::Forbidden(said) if said == "the caller may not write the row"),
        );
    }

    #[test]
    fn a_unique_violation_is_a_conflict_and_a_vanished_row_is_not_found() {
        match ServiceError::from(taken_email()) {
            ServiceError::Conflict(said) => {
                assert_eq!(said, "the request conflicts with an existing record");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        let problem = nest_rs_core::ToProblem::to_problem(&ServiceError::from(taken_email()))
            .expect("a conflict says its problem");
        assert_eq!(problem.status(), 409);
        assert_eq!(
            problem.errors(),
            None,
            "the conversion never sees the input, so it names no field",
        );
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
            let service = error_message(&ServiceError::Db(RepoError::from(err)));
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
    fn a_bare_database_error_rendered_as_text_is_redacted_to_its_sentence() {
        let error = bad_uuid();
        let displayed = error.to_string();
        assert!(displayed.contains("secret-123"), "{displayed}");
        let redacted = nest_rs_core::DecodeError::redact(&displayed, Some(&error));
        assert_eq!(redacted, "Execution Error: database error 22P02");
    }

    /// A rule's failure as `validator` hands it over: its parameters holding the
    /// rejected value, as `bad_uuid`'s message does.
    fn short_password() -> ValidationErrors {
        let mut rule = validator::ValidationError::new("length");
        rule.add_param("value".into(), &"secret-123");
        let mut errors = ValidationErrors::new();
        errors.add("password", rule);
        errors
    }

    /// A `{:?}`, a `main` returning it and an `anyhow` report print each link as
    /// it is: each says what `error_message` says, and what the variant refused
    /// stays in the variant, for the `match` that reads it.
    #[test]
    fn no_reader_of_a_service_error_prints_what_it_refused() {
        for (error, sentence) in [
            (
                ServiceError::Validation(short_password()),
                "validation failed: password: Validation error: length",
            ),
            (
                ServiceError::Db(RepoError::from(bad_uuid())),
                "database error: Execution Error: database error 22P02",
            ),
        ] {
            assert_eq!(error_message(&error), sentence);
            let printed = nest_rs_testing::printed(&error);
            assert!(!printed.contains("secret-123"), "{printed}");
            let report = format!("{:?}", anyhow::Error::from(error));
            assert!(!report.contains("secret-123"), "{report}");
            let (_, cause) = sentence.split_once(": ").expect("a cause");
            assert!(report.contains(cause), "{report}");
        }
        assert_eq!(
            format!("{:?}", ServiceError::Db(RepoError::from(bad_uuid()))),
            "Db(Other(\"Execution Error: database error 22P02\"))"
        );
        assert_eq!(
            format!("{:?}", ServiceError::Validation(short_password())),
            "Validation(\"password: Validation error: length\")"
        );
    }

    /// An opaque failure's cause reads the same: a decode failure or a driver's error
    /// beneath the detail is said, by every reader, as `error_message` says it.
    #[test]
    fn no_reader_of_an_internal_failure_prints_what_its_cause_quotes() {
        let decode = || serde_json::from_str::<u64>(r#""secret-123""#).expect_err("not a number");
        let runtime = || match bad_uuid() {
            DbErr::Exec(runtime) => runtime,
            other => panic!("a refused statement: {other:?}"),
        };
        for (error, sentence) in [
            (
                ServiceError::internal_because("the reply did not decode", decode()),
                "internal error: the reply did not decode: invalid type: a string, expected u64 \
                 at line 1 column 12",
            ),
            (
                ServiceError::internal_because(
                    "the reply did not decode",
                    anyhow::Error::from(decode()).context("reading the reply"),
                ),
                "internal error: the reply did not decode: reading the reply: invalid type: a \
                 string, expected u64 at line 1 column 12",
            ),
            (
                ServiceError::internal_because("the export failed", bad_uuid()),
                "internal error: the export failed: Execution Error: database error 22P02",
            ),
            (
                ServiceError::from(AtomicError::Database(Box::new(runtime()))),
                "internal error: the atomic block failed: the atomic block could not be settled: \
                 database error 22P02",
            ),
        ] {
            assert_eq!(error_message(&error), sentence);
            let printed = nest_rs_testing::printed(&error);
            assert!(!printed.contains("secret-123"), "{printed}");
            let debug = format!("{error:?}");
            assert!(!debug.contains("secret-123"), "{debug}");
            let report = format!("{:?}", anyhow::Error::from(error));
            assert!(!report.contains("secret-123"), "{report}");
            let (_, cause) = sentence.split_once(": ").expect("a cause");
            assert!(report.contains(cause), "{report}");
        }
        assert_eq!(
            format!("{:?}", ServiceError::internal("could not sign the export")),
            "Internal(\"could not sign the export\")"
        );
    }

    /// A port's failure whose chain still holds a `DbErr`, as another driver may
    /// box one, is read through the `SafeLink` by its SQLSTATE by every reader,
    /// never the server's message nor its detail, which quote the row.
    #[test]
    fn no_reader_of_a_port_failure_prints_the_row_its_database_refused() {
        fn read_by_all(err: impl Error + Send + Sync + 'static, sentence: &str) {
            assert_eq!(error_message(&err), sentence);
            let printed = nest_rs_testing::printed(&err);
            assert!(!printed.contains("ada@example.com"), "{printed}");
            let report = format!("{:?}", anyhow::Error::new(err));
            assert!(!report.contains("ada@example.com"), "{report}");
            assert!(report.contains("database error 23505"), "{report}");
        }
        read_by_all(
            AtomicError::Database(Box::new(taken_email())),
            "the atomic block could not be settled: Execution Error: database error 23505 on \
             constraint \"users_email_key\", table \"users\"",
        );
        read_by_all(
            nest_rs_database::OutboxError::Database(Box::new(taken_email())),
            "the outbox store's database failed: Execution Error: database error 23505 on \
             constraint \"users_email_key\", table \"users\"",
        );
        read_by_all(
            nest_rs_queue::QueueError::Outbox(nest_rs_database::OutboxError::Database(Box::new(
                taken_email(),
            ))),
            "the unit's outbox refused the job's record: the outbox store's database failed: \
             Execution Error: database error 23505 on constraint \"users_email_key\", table \"users\"",
        );
    }

    /// `#[error(transparent)]` hands out the error's own `source()`, skipping the
    /// error itself: what an operator's line says of the failure is that link's.
    #[test]
    fn a_wrapper_written_transparent_says_the_cause_as_the_error_does() {
        #[derive(Debug, thiserror::Error)]
        #[error(transparent)]
        struct Wrapping(#[from] ServiceError);

        for (error, said) in [
            (
                ServiceError::Validation(short_password()),
                "validation failed: password: Validation error: length",
            ),
            (
                ServiceError::Db(RepoError::from(bad_uuid())),
                "database error: Execution Error: database error 22P02",
            ),
            (
                ServiceError::internal_because("the export failed", bad_uuid()),
                "internal error: the export failed: Execution Error: database error 22P02",
            ),
        ] {
            let wrapping = Wrapping(error);
            assert_eq!(error_message(&wrapping), said);
            let printed = nest_rs_testing::printed(&wrapping);
            assert!(!printed.contains("secret-123"), "{printed}");
        }
    }

    #[test]
    fn a_validation_failure_with_no_field_says_its_sentence_alone() {
        let empty = ServiceError::Validation(ValidationErrors::new());
        assert_eq!(error_message(&empty), "validation failed");
    }

    #[test]
    fn what_carries_no_value_keeps_its_own_sentence() {
        for err in [
            DbErr::RecordNotFound("widget 7".into()),
            DbErr::RecordNotInserted,
            DbErr::RecordNotUpdated,
            DbErr::Custom("cursor column is not a uuid".into()),
            DbErr::Migration("We Don't Do That Here".into()),
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
        let err = ServiceError::Db(RepoError::from(DbErr::Custom(
            "SELECT password_hash FROM user".into(),
        )));
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
    fn an_internal_failure_says_its_detail_and_cause_and_stays_clone() {
        let refused = std::io::Error::other("the key store refused the connection");
        let err = ServiceError::internal_because("could not sign the export", refused);
        let copy = err.clone();
        for err in [&err, &copy] {
            let said = err.source().expect("the failure is the source");
            assert_eq!(
                said.to_string(),
                "could not sign the export: the key store refused the connection"
            );
            assert!(said.source().is_none(), "the sentence holds the chain");
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
        assert_eq!(
            bare.source().map(ToString::to_string).as_deref(),
            Some("could not sign the export"),
        );
    }

    /// The sentence reads against the constant above it, as the chain did: a blank
    /// detail is left out, and one repeating the constant is said once.
    #[test]
    fn a_blank_detail_or_one_repeating_the_constant_says_what_it_adds() {
        let cause = || std::io::Error::other("boom");
        for (error, sentence) in [
            (ServiceError::internal(""), "internal error"),
            (ServiceError::internal("  "), "internal error"),
            (ServiceError::internal("internal error"), "internal error"),
            (
                ServiceError::internal_because("", cause()),
                "internal error: boom",
            ),
            (
                ServiceError::internal_because("internal error", cause()),
                "internal error: boom",
            ),
            (
                ServiceError::internal_because("internal error: x", cause()),
                "internal error: internal error: x: boom",
            ),
            (
                ServiceError::internal_because("x", cause()),
                "internal error: x: boom",
            ),
        ] {
            assert_eq!(error_message(&error), sentence, "{error:?}");
        }
    }

    /// Each opaque failure whose chain holds an upstream's decode failure.
    pub(super) fn opaque_over_a_decode_failure() -> [ServiceError; 2] {
        let upstream =
            || serde_json::from_str::<u64>(r#""sk_live_secret""#).expect_err("not a number");
        [
            ServiceError::internal_because("the upstream reply did not decode", upstream()),
            ServiceError::internal_because(
                "the upstream reply did not decode",
                anyhow::Error::from(upstream()).context("reading the reply"),
            ),
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
        assert_eq!(filed.len(), 2, "{:#?}", logs.events());
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
            ServiceError::Db(repo) => assert_eq!(repo.to_string(), "Custom Error: connection lost"),
            other => panic!("expected Db, got {other:?}"),
        }
    }

    /// A migration's refusal is its own sentence, never the server's: the migrate
    /// binary, reporting through `RepoError`, says why it stopped.
    #[test]
    fn a_migration_s_refusal_keeps_its_sentence_as_a_repo_error() {
        let sea_orm_s_default_down = DbErr::Migration("We Don't Do That Here".into());
        let report = format!(
            "{:?}",
            anyhow::Error::new(RepoError::from(sea_orm_s_default_down))
        );
        assert_eq!(report, "Migration Error: We Don't Do That Here");
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
        use nest_rs_core::{ToProblem, code};

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
            Some((400, code::INVALID_ARGUMENT, "validation failed".into()))
        );
        for opaque in [
            ServiceError::Db(DbErr::Custom("relation \"posts\" does not exist".into()).into()),
            ServiceError::internal("hash failed"),
        ] {
            assert_eq!(opaque.to_problem(), None, "{opaque:?}");
        }
    }

    /// A validation failure's field map is its problem's, so every edge
    /// answers it under `errors`, not HTTP alone.
    #[test]
    fn a_validation_failure_s_problem_carries_its_field_map() {
        use nest_rs_core::ToProblem;

        let mut errors = ValidationErrors::new();
        errors.add("email", validator::ValidationError::new("email"));
        let failure = ServiceError::Validation(errors);
        let problem = failure.to_problem().expect("a client error");
        assert_eq!(problem.errors(), failure.field_errors().as_ref());
        assert!(
            problem
                .errors()
                .is_some_and(|fields| fields.get("email").is_some()),
            "{problem:?}"
        );
        assert_eq!(
            ServiceError::invalid("empty")
                .to_problem()
                .and_then(|problem| problem.errors().cloned()),
            None,
            "only a validation failure carries a field map",
        );
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

//! `ServiceError` stays unprefixed: it is written in every app service's
//! signature, like `Service` itself.

use sea_orm::DbErr;
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
    /// A `Repo`/ORM query failed. The `DbErr` detail stays for `tracing`; the
    /// wire sees a generic message.
    #[error("database error")]
    Db(#[from] DbErr),
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

#[cfg(feature = "http")]
pub use http::crud_error;

#[cfg(feature = "http")]
mod http {
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
        let (kind, detail): (&str, &dyn std::fmt::Display) = match err {
            ServiceError::Db(e) => ("db", e),
            ServiceError::Masking(e) => ("masking", e),
            ServiceError::Internal(e) => ("internal", e),
            _ => return,
        };
        tracing::error!(
            target: crate::target::ORM,
            kind,
            detail = %detail,
            "service error surfaced as 500",
        );
    }

    /// Map a `#[crud]` write failure to its status: a unique-constraint violation
    /// is a 409, a create the ability re-check rolled back (`RecordNotInserted`)
    /// a 403, a row that vanished before the write a 404; any other `DbErr` is
    /// a logged 500 with an empty body. Called by the `#[crud]` expansion.
    pub fn crud_error(err: sea_orm::DbErr) -> poem::Error {
        use sea_orm::{DbErr, SqlErr};

        let sql_err = err.sql_err();
        let status = match sql_err {
            Some(SqlErr::UniqueConstraintViolation(_)) => StatusCode::CONFLICT,
            _ => match err {
                DbErr::RecordNotInserted => StatusCode::FORBIDDEN,
                DbErr::RecordNotUpdated | DbErr::RecordNotFound(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            },
        };
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(
                target: crate::target::ORM,
                kind = "db",
                detail = %err,
                "crud operation failed",
            );
        }
        poem::Error::from_status(status)
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
                event.field("detail").is_some_and(|d| d.contains("posts")),
                "the cause the body withholds is the whole point, got {:?}",
                event.fields,
            );
        }

        #[test]
        fn a_crud_500_emits_the_cause_and_the_mapped_statuses_do_not() {
            let logs = nest_rs_testing::LogCapture::install();

            let mapped = crud_error(sea_orm::DbErr::RecordNotInserted);
            assert_eq!(mapped.status(), StatusCode::FORBIDDEN);

            let unexpected = crud_error(sea_orm::DbErr::Custom("deadlock detected".into()));
            assert_eq!(unexpected.status(), StatusCode::INTERNAL_SERVER_ERROR);

            let event = logs.expect_one(crate::target::ORM, "crud operation failed");
            assert_eq!(event.level, "error");
            assert_eq!(event.field("kind").as_deref(), Some("db"));
            assert!(
                event
                    .field("detail")
                    .is_some_and(|d| d.contains("deadlock")),
                "only the unmapped failure is logged, and it carries its cause: {:?}",
                event.fields,
            );
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
    use super::*;

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

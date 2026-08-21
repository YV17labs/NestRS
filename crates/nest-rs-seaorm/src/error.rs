//! Shared service error: the failure modes a `CrudService` method returns —
//! plumbing (`Repo` call, `validator` derive, masking) plus the business-rule
//! outcomes a service expresses against its data — together with their HTTP
//! mapping. Domain-specific *wire* contracts (an opaque credential rejection,
//! an RFC 6749 code) still live in their own crates
//! (`nest_rs_authn::CredentialError`, `nest_rs_oauth_server::TokenError`); features
//! never re-define those.
//!
//! The business variants ([`Invalid`](ServiceError::Invalid),
//! [`Conflict`](ServiceError::Conflict), [`Forbidden`](ServiceError::Forbidden),
//! [`NotFound`](ServiceError::NotFound)) carry a **client-facing** message and
//! map to the matching 4xx — a service signals "empty body" or "insufficient
//! balance" without hand-rolling a per-feature error or, worse, masking it as a
//! `DbErr` (HTTP 500). The opaque variants ([`Db`](ServiceError::Db),
//! [`Internal`](ServiceError::Internal), [`Masking`](ServiceError::Masking))
//! keep a constant wire string (detail stays for `tracing`); `Validation`
//! forwards through so the field errors stay structured. `Display` is what both
//! Poem's `ResponseError` and the WS reply put on the wire.
//!
//! **Naming decision (kept, do not re-flag):** this stays `ServiceError`, not a
//! concern-prefixed `SeaOrmError`. It is developer vocabulary written in every
//! app service signature (`Result<T, ServiceError>`) and is role-named like
//! `Service` itself; a prefix would hurt exactly the ergonomics the framework
//! sells, so the concern-prefix rule (`RedisError`, `StorageError`) is
//! deliberately not applied here.

use sea_orm::DbErr;
use validator::ValidationErrors;

/// Failure modes shared by every service method. The plumbing variants come
/// from `Repo`/`validator`/masking; the business variants are constructed by
/// services via [`ServiceError::invalid`] & friends.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum ServiceError {
    /// Edge validation (`validator`) rejected the input — maps to a 400.
    ///
    /// The message is the constant every transport already uses for a pipe
    /// rejection; the offending fields ride structurally, under `errors`
    /// ([`field_errors`](Self::field_errors)). `transparent` used to render
    /// `validator`'s own `Debug` payload — `name: Validation error: length
    /// [{"min": Number(1), "value": String("")}]` — which is unreadable, not
    /// programmable, and echoes the **rejected value** back out: a too-short
    /// password or a malformed token into every log and transcript that
    /// captures the line.
    #[error("validation failed")]
    Validation(#[from] ValidationErrors),
    /// A `Repo`/ORM query failed. The `DbErr` detail stays for `tracing`; the
    /// wire sees a generic message.
    #[error("database error")]
    Db(#[from] DbErr),
    /// Response masking could not reconcile a loaded row into its wire DTO.
    /// Fail closed (500) rather than leak an unmasked row — the detail stays
    /// for `tracing`, never the wire. Carries a `String` (not the source
    /// `serde_json::Error`) so the enum stays `Clone` for dataloader plumbing.
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
    #[error("internal error")]
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
    /// as the JSON every transport ships under `errors`.
    ///
    /// One accessor so no transport can disagree about the shape, and it routes
    /// through `nest_rs_pipes::validation_details` so none can disagree about
    /// the **policy** either: a raw `serde_json::to_value` keeps `params.value`,
    /// the rejected input, and would put a too-short password or a malformed
    /// token in every log and transcript that captures the response.
    pub fn field_errors(&self) -> Option<serde_json::Value> {
        match self {
            Self::Validation(errors) => Some(nest_rs_pipes::validation_details(errors)),
            _ => None,
        }
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

        /// Render the failure as the single RFC-9457 `application/problem+json`
        /// envelope. `Display` (the wire-safe string — a constant for the opaque
        /// 5xx variants, the authored message for the 4xx business variants) is
        /// the `detail`, so a `DbErr`/internal message never reaches the wire;
        /// `Validation` additionally carries its field errors as an `errors`
        /// extension member.
        ///
        /// This is also the single place the opaque variants' detail is
        /// **logged**: their whole contract is "wire sees a constant, `tracing`
        /// sees the cause", and a 5xx nobody can grep for is the same defect as
        /// no error handling at all.
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

    /// Emit the cause behind an opaque 5xx. The 4xx business variants are the
    /// client's own doing and already carry their message on the wire, so they
    /// stay silent here.
    fn log_opaque(err: &ServiceError) {
        // Borrow the cause rather than stringify it: the macro formats inside
        // its own `if enabled`, so a filtered-out event costs nothing.
        let (kind, detail): (&str, &dyn std::fmt::Display) = match err {
            ServiceError::Db(e) => ("db", e),
            ServiceError::Masking(e) => ("masking", e),
            ServiceError::Internal(e) => ("internal", e),
            _ => return,
        };
        tracing::error!(
            target: crate::TARGET,
            kind,
            detail = %detail,
            "service error surfaced as 500",
        );
    }

    /// Map a `#[crud]` write failure to the status it deserves instead of a
    /// blanket 500: a unique-constraint violation is a 409, a create the
    /// ability re-check rolled back (`RecordNotInserted`) is a 403, a row that
    /// vanished between the access check and the write is a 404. Only a
    /// genuinely unexpected `DbErr` is a 500 — logged in full here, shipped as
    /// an empty body so the driver message never reaches the client.
    ///
    /// Called by the `#[crud]` expansion; hand-written handlers use
    /// [`ServiceError`] directly.
    #[doc(hidden)]
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
                target: crate::TARGET,
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

        /// The opaque half of the same contract: the body carries no cause, so
        /// the event has to.
        ///
        /// A `DbErr` names tables, columns and sometimes values, which is why it
        /// never reaches the client — and why an operator has exactly one place
        /// to read it. `kind` is what separates a driver failure from a masking
        /// failure, and those lead to entirely different files.
        #[test]
        fn a_500_emits_the_cause_it_refuses_to_ship() {
            let logs = nest_rs_testing::LogCapture::install();
            let _ = ServiceError::Db(sea_orm::DbErr::Custom(
                "relation \"posts\" does not exist".into(),
            ))
            .as_response();

            let event = logs.expect_one("nest_rs::orm", "service error surfaced as 500");
            assert_eq!(event.level, "error");
            assert_eq!(event.field("kind").as_deref(), Some("db"));
            assert!(
                event.field("detail").is_some_and(|d| d.contains("posts")),
                "the cause the body withholds is the whole point, got {:?}",
                event.fields,
            );
        }

        /// `#[crud]`'s own mapper, and the same split one layer down: only the
        /// blanket 500 is logged, because every other status *is* the
        /// explanation. A `409` says unique-constraint, a `403` says the create
        /// re-check rolled it back — an unexpected `DbErr` says nothing at all,
        /// and ships an empty body.
        #[test]
        fn a_crud_500_emits_the_cause_and_the_mapped_statuses_do_not() {
            let logs = nest_rs_testing::LogCapture::install();

            let mapped = crud_error(sea_orm::DbErr::RecordNotInserted);
            assert_eq!(mapped.status(), StatusCode::FORBIDDEN);

            let unexpected = crud_error(sea_orm::DbErr::Custom("deadlock detected".into()));
            assert_eq!(unexpected.status(), StatusCode::INTERNAL_SERVER_ERROR);

            let event = logs.expect_one("nest_rs::orm", "crud operation failed");
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

        /// And a 4xx stays silent: it is the client's own doing and already
        /// carries its message on the wire, so logging it would bury the 5xx
        /// lines that matter under traffic nobody needs to act on.
        #[test]
        fn a_business_4xx_emits_nothing() {
            let logs = nest_rs_testing::LogCapture::install();
            let _ = ServiceError::not_found("no such widget").as_response();
            assert!(
                logs.find("nest_rs::orm", "service error surfaced as 500")
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
            // Only the constant wire string — never the SQL that leaked schema.
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
        // The inner `DbErr` may name a column, table, or SQL state that
        // leaks schema details — the wire string must not.
        let err = ServiceError::Db(DbErr::Custom("SELECT password_hash FROM user".into()));
        assert_eq!(err.to_string(), "database error");
    }

    #[test]
    fn internal_display_is_wire_safe_constant() {
        // Like `Db`, an internal failure keeps its detail for `tracing` only.
        let err = ServiceError::internal("stripe key rejected: sk_live_… ");
        assert_eq!(err.to_string(), "internal error");
    }

    #[test]
    fn business_variants_forward_their_message() {
        // 4xx messages are authored, non-sensitive, and meant for the client.
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

    // `serde_json::to_value(ValidationErrors)` keeps `params.value` — the
    // rejected input. Every transport reads the field errors through
    // `field_errors`, so the redaction has to hold there, not at each renderer.
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
        assert!(rendered.contains("min"), "and its bound: {rendered}");
        assert!(
            !rendered.contains("hunter2"),
            "the submitted value must never ride back out: {rendered}",
        );
    }
}

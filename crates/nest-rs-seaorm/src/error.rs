//! Shared service error: the failure modes a `CrudService` method returns —
//! plumbing (`Repo` call, `validator` derive, masking) plus the business-rule
//! outcomes a service expresses against its data — together with their HTTP
//! mapping. Domain-specific *wire* contracts (an opaque credential rejection,
//! an RFC 6749 code) still live in their own crates
//! (`nest_rs_authn::CredentialError`, `nest_rs_authn::TokenError`); features
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

use sea_orm::DbErr;
use validator::ValidationErrors;

/// Failure modes shared by every service method. The plumbing variants come
/// from `Repo`/`validator`/masking; the business variants are constructed by
/// services via [`ServiceError::invalid`] & friends.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationErrors),
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
}

#[cfg(feature = "http")]
mod http {
    use poem::error::ResponseError;
    use poem::http::StatusCode;

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
}

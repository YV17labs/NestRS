//! Shared service error: the plumbing failure modes every feature returns from
//! a `CrudService` method (`Repo` call, `validator` derive) plus their HTTP
//! mapping. Domain-specific failures (an opaque credential rejection, an
//! RFC 6749 wire code) live in their respective framework crates
//! (`nest_rs_authn::CredentialError`, `nest_rs_authn::TokenError`) — features
//! never re-define them.
//!
//! `Display` for `Db` is the fixed string `"database error"` — the inner
//! `DbErr` stays for `tracing`, never the wire (Poem's `ResponseError` and the
//! WS reply both call `Display`). `Validation` forwards through so the field
//! errors stay structured.

use sea_orm::DbErr;
use validator::ValidationErrors;

/// Failure modes shared by every service method that goes through `Repo` or
/// validates input. Domain-specific failures (opaque credential rejection,
/// RFC 6749 wire codes) keep their own types alongside this one.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationErrors),
    #[error("database error")]
    Db(#[from] DbErr),
}

#[cfg(feature = "http")]
mod http {
    use poem::error::ResponseError;
    use poem::http::StatusCode;

    use super::ServiceError;

    impl ResponseError for ServiceError {
        fn status(&self) -> StatusCode {
            match self {
                ServiceError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
                ServiceError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
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

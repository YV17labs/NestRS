use sea_orm::DbErr;
use validator::ValidationErrors;

#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid credentials")]
pub struct CredentialError;

/// `Display` for [`UserError::Db`] is a fixed, wire-safe message — the inner
/// `DbErr` stays structured for logs but never reaches the wire (Poem's
/// `ResponseError` and the WS reply both use `Display`).
#[derive(Debug, Clone, thiserror::Error)]
pub enum UserError {
    #[error(transparent)]
    Validation(#[from] ValidationErrors),
    #[error("database error")]
    Db(#[from] DbErr),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_display_is_wire_safe_constant() {
        // The inner `DbErr` may name a column, table, or SQL state that
        // leaks schema details — the wire string must not.
        let err = UserError::Db(DbErr::Custom("SELECT password_hash FROM user".into()));
        assert_eq!(err.to_string(), "database error");
    }

    #[test]
    fn db_from_db_err_does_not_lose_inner() {
        let inner = DbErr::Custom("connection lost".into());
        let err: UserError = inner.into();
        match err {
            UserError::Db(DbErr::Custom(msg)) => assert_eq!(msg, "connection lost"),
            other => panic!("expected Db, got {other:?}"),
        }
    }

    #[test]
    fn validation_from_validation_errors_propagates_field_errors() {
        let mut errs = ValidationErrors::new();
        errs.add(
            "email",
            validator::ValidationError::new("not_an_email"),
        );
        let err: UserError = errs.into();
        match err {
            UserError::Validation(v) => assert!(v.field_errors().contains_key("email")),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn credential_error_display_does_not_leak_detail() {
        assert_eq!(CredentialError.to_string(), "invalid credentials");
    }
}

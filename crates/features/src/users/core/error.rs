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

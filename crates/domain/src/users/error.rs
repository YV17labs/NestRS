use sea_orm::DbErr;
use validator::ValidationErrors;

/// Wrong email or password — always the same message on the wire.
#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid credentials")]
pub struct CredentialError;

/// A failure creating, registering, or batch-loading users.
#[derive(Debug, Clone, thiserror::Error)]
pub enum UserError {
    #[error(transparent)]
    Validation(#[from] ValidationErrors),
    #[error(transparent)]
    Db(#[from] DbErr),
}

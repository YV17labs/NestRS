use sea_orm::DbErr;

/// `Display` is wire-safe; the inner `DbErr` stays for logs (same discipline
/// as [`crate::users::UserError`]).
#[derive(Debug, Clone, thiserror::Error)]
pub enum OrgError {
    #[error("database error")]
    Db(#[from] DbErr),
}

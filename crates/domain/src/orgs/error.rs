use sea_orm::DbErr;

/// A failure batch-loading orgs.
///
/// `Display` is a fixed, wire-safe message — the inner `DbErr` is kept
/// structured for downstream observability but never reaches the wire (see
/// [`crate::users::UserError`] for the discipline).
#[derive(Debug, Clone, thiserror::Error)]
pub enum OrgError {
    #[error("database error")]
    Db(#[from] DbErr),
}

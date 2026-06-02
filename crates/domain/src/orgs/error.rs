use sea_orm::DbErr;

/// A failure batch-loading orgs.
#[derive(Debug, Clone, thiserror::Error)]
pub enum OrgError {
    #[error(transparent)]
    Db(#[from] DbErr),
}

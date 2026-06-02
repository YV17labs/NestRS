use sea_orm::DbErr;
use validator::ValidationErrors;

/// Wrong email or password — always the same message on the wire.
#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid credentials")]
pub struct CredentialError;

/// A failure creating, registering, or batch-loading users.
///
/// `Display` for [`UserError::Db`] is a fixed, wire-safe message — the inner
/// `DbErr` is kept structured for downstream observability (`Debug`-format
/// logging captures the full SeaORM error) but never reaches the wire. The
/// `#[messages]`-generated WebSocket reply and Poem's default `ResponseError`
/// body both use `Display`, so leaks at the transport boundary are
/// structurally impossible. The `Validation` variant stays transparent: those
/// messages are user-input feedback, intentionally surfaced.
#[derive(Debug, Clone, thiserror::Error)]
pub enum UserError {
    #[error(transparent)]
    Validation(#[from] ValidationErrors),
    #[error("database error")]
    Db(#[from] DbErr),
}

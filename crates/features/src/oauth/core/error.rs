/// A token request that cannot be fulfilled. [`Display`](std::fmt::Display) yields OAuth2 error codes.
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("unsupported_grant_type")]
    UnsupportedGrant,
    #[error("invalid_scope")]
    InvalidScope,
    #[error("invalid_credentials")]
    InvalidCredentials,
    #[error("server_error")]
    Sign(#[source] anyhow::Error),
}

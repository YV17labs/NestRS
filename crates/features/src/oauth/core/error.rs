/// `Display` yields OAuth2 error codes (RFC 6749).
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

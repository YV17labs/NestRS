//! Authentication failures, rendered as HTTP 401 challenges.

use poem::http::{StatusCode, header};
use poem::{IntoResponse, Response};

/// Opaque "wrong credentials" failure for any password-login path.
///
/// Returned by services that verify a password against a stored hash: missing
/// user, missing hash, wrong password, and DB unreachable all collapse into
/// this single variant so timing and wire string never distinguish them.
/// `Display` is the fixed `"invalid credentials"`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid credentials")]
pub struct CredentialError;

/// Why authentication did not establish an identity.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing credentials")]
    MissingCredentials,
    #[error("invalid token")]
    InvalidToken,
    #[error("invalid token signature")]
    InvalidSignature,
    #[error("invalid token algorithm")]
    InvalidAlgorithm,
    #[error("token not yet valid")]
    NotYetValid,
    #[error("token expired")]
    Expired,
    /// Strategy-specific or configuration failures. The message is for logs, not the client body.
    #[error("authentication failed: {0}")]
    Failed(String),
}

impl AuthError {
    /// Message safe to return in an HTTP 401 body (no strategy/configuration detail).
    pub fn client_message(&self) -> String {
        match self {
            Self::Failed(_) => "authentication failed".into(),
            Self::MissingCredentials => "missing credentials".into(),
            _ => "invalid token".into(),
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        if let Self::Failed(ref detail) = self {
            tracing::warn!(target: "nest_rs::authn", detail = %detail, "authentication failed");
        }
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Bearer")
            .body(self.client_message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_error_display_does_not_leak_detail() {
        assert_eq!(CredentialError.to_string(), "invalid credentials");
    }
}

//! Token endpoint failures (RFC 6749).
//!
//! `Display` yields the wire code an OAuth2 client reads in the error
//! response body — a rename here breaks every conforming client.

use poem::error::ResponseError;
use poem::http::StatusCode;

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("unsupported_grant_type")]
    UnsupportedGrant,
    #[error("invalid_scope")]
    InvalidScope,
    #[error("invalid_credentials")]
    InvalidCredentials,
    /// Internal signing failure. `Display` is the opaque RFC 6749
    /// `server_error`; the source stays attached for `tracing`.
    #[error("server_error")]
    Sign(#[source] anyhow::Error),
}

impl ResponseError for TokenError {
    fn status(&self) -> StatusCode {
        match self {
            TokenError::Sign(_) => StatusCode::INTERNAL_SERVER_ERROR,
            TokenError::InvalidCredentials => StatusCode::UNAUTHORIZED,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    // RFC 6749 wire codes — a rename here breaks every conforming client.
    #[test]
    fn display_emits_rfc6749_codes() {
        assert_eq!(TokenError::UnsupportedGrant.to_string(), "unsupported_grant_type");
        assert_eq!(TokenError::InvalidScope.to_string(), "invalid_scope");
        assert_eq!(
            TokenError::InvalidCredentials.to_string(),
            "invalid_credentials",
        );
    }

    #[test]
    fn sign_display_hides_internal_detail() {
        let inner = anyhow::anyhow!("secret key path /etc/keys/oauth.pem unreadable");
        let err = TokenError::Sign(inner);
        assert_eq!(err.to_string(), "server_error");
        // The internal cause is reachable for tracing but never on the wire.
        assert!(err.source().is_some(), "source preserved for logs");
    }

    #[test]
    fn unsupported_grant_is_400() {
        assert_eq!(TokenError::UnsupportedGrant.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn invalid_scope_is_400() {
        assert_eq!(TokenError::InvalidScope.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn invalid_credentials_is_401() {
        assert_eq!(
            TokenError::InvalidCredentials.status(),
            StatusCode::UNAUTHORIZED,
        );
    }

    #[test]
    fn sign_is_500() {
        let err = TokenError::Sign(anyhow::anyhow!("boom"));
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}

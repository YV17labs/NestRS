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
}

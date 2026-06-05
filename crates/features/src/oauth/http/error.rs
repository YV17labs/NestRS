use poem::error::ResponseError;
use poem::http::StatusCode;

use crate::oauth::core::TokenError;

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
    use super::*;

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

use poem::error::ResponseError;
use poem::http::StatusCode;

use crate::oauth::error::TokenError;

impl ResponseError for TokenError {
    fn status(&self) -> StatusCode {
        match self {
            TokenError::Sign(_) => StatusCode::INTERNAL_SERVER_ERROR,
            TokenError::InvalidCredentials => StatusCode::UNAUTHORIZED,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

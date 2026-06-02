use poem::error::ResponseError;
use poem::http::StatusCode;

use super::super::core::OrgError;

impl ResponseError for OrgError {
    fn status(&self) -> StatusCode {
        match self {
            OrgError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

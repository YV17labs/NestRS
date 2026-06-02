use poem::error::ResponseError;
use poem::http::StatusCode;

use crate::users::error::UserError;

impl ResponseError for UserError {
    fn status(&self) -> StatusCode {
        match self {
            UserError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            UserError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

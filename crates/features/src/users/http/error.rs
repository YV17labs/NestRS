use poem::error::ResponseError;
use poem::http::StatusCode;

use super::super::core::UserError;

impl ResponseError for UserError {
    fn status(&self) -> StatusCode {
        match self {
            UserError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            UserError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::DbErr;
    use validator::ValidationErrors;

    use super::*;

    #[test]
    fn validation_is_422() {
        let err = UserError::Validation(ValidationErrors::new());
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn db_is_500() {
        let err = UserError::Db(DbErr::Custom("boom".into()));
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}

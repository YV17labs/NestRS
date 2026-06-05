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

#[cfg(test)]
mod tests {
    use sea_orm::DbErr;

    use super::*;

    #[test]
    fn db_is_500() {
        assert_eq!(
            OrgError::Db(DbErr::Custom("boom".into())).status(),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
}

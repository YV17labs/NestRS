use nest_rs::core::problem::code;
use nest_rs::core::{Problem, ToProblem};
use nest_rs::seaorm::ServiceError;
use poem::error::ResponseError;
use poem::http::StatusCode;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PostError {
    #[error("post {id} is already published")]
    AlreadyPublished { id: Uuid },
    #[error(transparent)]
    Service(#[from] ServiceError),
}

impl ResponseError for PostError {
    fn status(&self) -> StatusCode {
        match self {
            PostError::AlreadyPublished { .. } => StatusCode::CONFLICT,
            PostError::Service(svc) => svc.status(),
        }
    }
}

impl ToProblem for PostError {
    fn to_problem(&self) -> Option<Problem> {
        match self {
            PostError::AlreadyPublished { .. } => {
                Some(Problem::new(409, code::CONFLICT).with_detail(self.to_string()))
            }
            PostError::Service(svc) => svc.to_problem(),
        }
    }
}

use nest_rs_seaorm::ServiceError;
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

use nest_rs::core::problem::code;
use nest_rs::core::{Problem, ToProblem};
use nest_rs::http::{ResponseError, StatusCode};
use nest_rs::queue::QueueError;
use nest_rs::seaorm::ServiceError;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PostError {
    #[error("post {id} is already published")]
    AlreadyPublished { id: Uuid },
    #[error(transparent)]
    Service(#[from] ServiceError),
    #[error("the post's notification could not be queued")]
    Queue(#[from] QueueError),
}

impl ResponseError for PostError {
    fn status(&self) -> StatusCode {
        match self {
            PostError::AlreadyPublished { .. } => StatusCode::CONFLICT,
            PostError::Service(svc) => svc.status(),
            PostError::Queue(_) => StatusCode::INTERNAL_SERVER_ERROR,
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
            PostError::Queue(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use nest_rs::core::ToProblem;
    use nest_rs::http::{ResponseError, StatusCode};
    use nest_rs::queue::QueueError;

    use super::PostError;

    #[test]
    fn a_notification_the_queue_refused_answers_an_opaque_500() {
        let leaky = std::io::Error::other("connect redis://cache.internal:6379 refused");
        let err = PostError::from(QueueError::backend(leaky));

        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.to_problem().is_none());
        assert_eq!(
            err.to_string(),
            "the post's notification could not be queued"
        );
        assert!(err.source().is_some());
    }
}

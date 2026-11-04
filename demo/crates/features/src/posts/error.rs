use nest_rs::core::{Problem, ToProblem, code};
use nest_rs::queue::QueueError;
use nest_rs::seaorm::ServiceError;
use uuid::Uuid;

const ALREADY_PUBLISHED: Problem = Problem::new(409, code::CONFLICT).with_type(
    "https://nestrs.dev/problems/post-already-published",
    "Post already published",
);

#[derive(Debug, thiserror::Error)]
pub enum PostError {
    #[error("post {id} is already published")]
    AlreadyPublished { id: Uuid },
    #[error(transparent)]
    Service(#[from] ServiceError),
    #[error("the post's notification could not be queued")]
    Queue(#[from] QueueError),
}

impl ToProblem for PostError {
    fn to_problem(&self) -> Option<Problem> {
        match self {
            PostError::AlreadyPublished { .. } => {
                Some(ALREADY_PUBLISHED.with_detail(self.to_string()))
            }
            PostError::Service(svc) => svc.to_problem(),
            PostError::Queue(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use nest_rs::core::{ToProblem, code};
    use nest_rs::queue::QueueError;
    use uuid::Uuid;

    use super::PostError;

    #[test]
    fn a_post_published_twice_answers_its_problem_type() {
        let err = PostError::AlreadyPublished { id: Uuid::nil() };
        let problem = err.to_problem().expect("a client error");

        assert_eq!((problem.status(), problem.code()), (409, code::CONFLICT));
        assert_eq!(
            problem.type_uri(),
            Some("https://nestrs.dev/problems/post-already-published")
        );
        assert_eq!(problem.title(), Some("Post already published"));
        assert_eq!(problem.detail(), Some(err.to_string().as_str()));
    }

    #[test]
    fn a_notification_the_queue_refused_answers_opaquely() {
        let leaky = std::io::Error::other("connect redis://cache.internal:6379 refused");
        let err = PostError::from(QueueError::backend(leaky));

        assert!(err.to_problem().is_none());
        assert_eq!(
            err.to_string(),
            "the post's notification could not be queued"
        );
        assert!(err.source().is_some());
    }
}

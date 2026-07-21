use nest_rs_seaorm::ServiceError;
use poem::error::ResponseError;
use poem::http::StatusCode;
use uuid::Uuid;

/// The publish flow's error. `AlreadyPublished` is the domain-specific wire
/// contract (a custom RFC 9457 problem type rendered by `PostProblemFilter`);
/// `Service` forwards a `Repo`/service failure so `publish` has one error type
/// while keeping `ServiceError`'s own opaque wire mapping.
#[derive(Debug, thiserror::Error)]
pub enum PostError {
    #[error("post {id} is already published")]
    AlreadyPublished { id: Uuid },
    #[error(transparent)]
    Service(#[from] ServiceError),
}

impl ResponseError for PostError {
    /// The status a route without `PostProblemFilter` would fall back to; the
    /// publish route hands `AlreadyPublished` to the filter for its custom
    /// problem body, and `Service` carries `ServiceError`'s own status.
    fn status(&self) -> StatusCode {
        match self {
            PostError::AlreadyPublished { .. } => StatusCode::CONFLICT,
            PostError::Service(svc) => svc.status(),
        }
    }
}

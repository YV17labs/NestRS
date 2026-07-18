use uuid::Uuid;

/// A posts-domain rule violation the caller can correct — distinct from
/// `nest_rs_seaorm::ServiceError` (infrastructure / DB failure). It is mapped to
/// an HTTP response in exactly one place, the `PostProblemFilter` exception
/// filter, which renders it as RFC 9457 `application/problem+json`.
#[derive(Debug, thiserror::Error)]
pub enum PostError {
    /// Publishing a post already in the `Published` state. The transition is a
    /// no-op, but silently succeeding would hide the fact from the caller, so it
    /// surfaces as a `409 Conflict` problem document instead.
    #[error("post {id} is already published")]
    AlreadyPublished {
        /// The post whose state already reached `Published`.
        id: Uuid,
    },
}

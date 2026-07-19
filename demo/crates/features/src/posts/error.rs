use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PostError {
    #[error("post {id} is already published")]
    AlreadyPublished { id: Uuid },
}

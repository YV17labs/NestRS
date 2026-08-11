use uuid::Uuid;

use super::entities::post::Post;

#[derive(Clone)]
pub struct PostPublishedEvent {
    pub post_id: Uuid,
    pub org_id: Uuid,
    pub post: Post,
}

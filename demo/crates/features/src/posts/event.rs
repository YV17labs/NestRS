use super::entities::post::Post;

#[derive(Clone)]
pub struct PostPublishedEvent {
    pub post: Post,
}

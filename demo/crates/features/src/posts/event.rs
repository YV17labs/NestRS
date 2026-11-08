use super::entities::post::Model;

#[derive(Clone)]
pub struct PostPublishedEvent {
    pub post: Model,
}

use uuid::Uuid;

#[derive(Clone)]
pub struct PostPublishedEvent {
    pub post_id: Uuid,
    pub org_id: Uuid,
    pub title: String,
}

use uuid::Uuid;

/// Published fact: a post was created in an org. A queue payload crosses a
/// process boundary; an event stays in-process, so it lives at the feature
/// port as a plain `Clone` struct (the bus enforces `Clone + Send + 'static`).
/// Consumers subscribe with `#[on_event]` — see the `notifications` slice.
#[derive(Clone)]
pub struct PostPublishedEvent {
    pub post_id: Uuid,
    pub org_id: Uuid,
    pub title: String,
}

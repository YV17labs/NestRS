use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::events::listeners;

use crate::posts::{PostFeed, PostPublishedEvent};

#[injectable]
pub struct PostsListener {
    #[inject]
    feed: Arc<PostFeed>,
}

#[listeners]
impl PostsListener {
    #[on_event]
    async fn on_post_published(&self, event: PostPublishedEvent) {
        self.feed.publish(event.post);
        tracing::debug!(
            target: "features::posts",
            post_id = %event.post_id,
            org_id = %event.org_id,
            "fanned a published post out to subscribers",
        );
    }
}

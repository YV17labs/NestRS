use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::events::listeners;
use nest_rs::queue::JobProducerExt;
use nest_rs::redis::QueueConnection;

use crate::notifications::{NotifyCommand, NotifyQueue};
use crate::posts::PostPublishedEvent;

#[injectable]
pub struct NotificationsListener {
    #[inject]
    queue: Arc<QueueConnection>,
}

#[listeners]
impl NotificationsListener {
    #[on_event]
    async fn on_post_published(&self, event: PostPublishedEvent) {
        let command = NotifyCommand {
            org_id: event.org_id,
            message: format!("Post \"{}\" was published", event.post.title),
        };
        match self.queue.push_to::<NotifyQueue>(command).await {
            Ok(()) => tracing::debug!(
                target: "features::notifications",
                post_id = %event.post_id,
                org_id = %event.org_id,
                "enqueued a publish notification for the worker",
            ),
            Err(error) => tracing::error!(
                target: "features::notifications",
                %error,
                post_id = %event.post_id,
                org_id = %event.org_id,
                "failed to enqueue a publish notification",
            ),
        }
    }
}

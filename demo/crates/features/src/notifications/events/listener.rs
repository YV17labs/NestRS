use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::events::listeners;
use nest_rs::queue::{JobProducer, JobProducerExt};

use crate::notifications::{NotifyCommand, NotifyQueue};
use crate::posts::PostPublishedEvent;

#[injectable]
pub struct NotificationsListener {
    #[inject]
    queue: Arc<dyn JobProducer>,
}

#[listeners]
impl NotificationsListener {
    #[on_event]
    async fn on_post_published(&self, event: PostPublishedEvent) {
        let command = NotifyCommand {
            org_id: event.org_id,
            message: format!("Post \"{}\" was published", event.post.title),
        };
        match self.queue.push(NotifyQueue, command, None).await {
            Ok(receipt) => tracing::debug!(
                target: "features::notifications",
                post_id = %event.post_id,
                org_id = %event.org_id,
                job_id = %receipt.id(),
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

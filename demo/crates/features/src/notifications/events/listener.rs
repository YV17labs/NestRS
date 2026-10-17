use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::events::listeners;
use nest_rs::queue::{JobProducer, JobProducerExt, QueueError};

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
    async fn on_post_published(&self, event: PostPublishedEvent) -> Result<(), QueueError> {
        let command = NotifyCommand {
            org_id: event.org_id,
            message: format!("Post \"{}\" was published", event.post.title),
        };
        let receipt = self.queue.push(NotifyQueue, command, None).await?;
        tracing::debug!(
            target: crate::notifications::TARGET,
            post_id = %event.post_id,
            org_id = %event.org_id,
            job_id = %receipt.id(),
            "enqueued a publish notification for the worker",
        );
        Ok(())
    }
}

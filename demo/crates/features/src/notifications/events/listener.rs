use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_events::listeners;
use nest_rs_queue::JobProducerExt;
use nest_rs_redis::QueueConnection;

use crate::notifications::{NotifyCommand, NotifyQueue};
use crate::posts::PostPublishedEvent;

/// Listener host for post-publication side effects. A plain provider (not a
/// service): it reacts to a fact, it is not the entity's DB gateway. A listener
/// runs **outside** any request — no ambient executor, no transaction — so it
/// must never touch the DB directly. Instead it stays a pure **producer**,
/// enqueuing a [`NotifyCommand`] the worker persists in its own system context.
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
            message: format!("Post \"{}\" was published", event.title),
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

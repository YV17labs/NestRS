use nest_rs_core::injectable;
use nest_rs_events::listeners;

use crate::posts::PostPublishedEvent;

/// Listener host for post-publication side effects. A plain provider (not a
/// service): it reacts to a fact, it is not the entity's DB gateway. Here it
/// logs; a real app would enqueue an email or a push notification.
#[injectable]
#[derive(Default)]
pub struct NotificationsListener;

#[listeners]
impl NotificationsListener {
    #[on_event]
    async fn on_post_published(&self, event: PostPublishedEvent) {
        tracing::info!(
            target: "features::notifications",
            post_id = %event.post_id,
            org_id = %event.org_id,
            title = %event.title,
            "notifying subscribers of a published post",
        );
    }
}

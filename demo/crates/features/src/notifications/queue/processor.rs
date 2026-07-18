use std::sync::Arc;

use anyhow::Result;
use nest_rs_core::injectable;
use nest_rs_queue::processor;

use crate::notifications::{NotificationsService, NotifyCommand, NotifyQueue};

/// Worker consumer for the `notifications` queue. The `WorkerDbContext`
/// installed by `DatabaseModule` gives each job a legitimate pool executor
/// (system context, no ability — unscoped by design), so the processor can
/// persist through the service + `Repo`.
#[injectable]
pub struct NotificationsProcessor {
    #[inject]
    svc: Arc<NotificationsService>,
}

#[processor]
impl NotificationsProcessor {
    #[process(queue = NotifyQueue, concurrency = 5, retries = 3)]
    async fn notify(&self, job: NotifyCommand) -> Result<()> {
        self.svc.persist(job).await?;
        Ok(())
    }
}

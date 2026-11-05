use std::sync::Arc;

use anyhow::Result;
use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

use crate::notifications::NotificationsService;

#[injectable]
pub struct NotificationsTasks {
    #[inject]
    svc: Arc<NotificationsService>,
}

#[scheduled]
impl NotificationsTasks {
    #[every("1h", replicas = "one")]
    async fn purge_expired(&self) -> Result<()> {
        self.svc.purge_expired().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::time::Duration;

    use nest_rs::schedule::{Replicas, ScheduledMethod, Trigger};

    use super::NotificationsTasks;

    #[test]
    fn the_purge_fires_hourly_on_one_replica() {
        let purge = nest_rs::core::inventory::iter::<ScheduledMethod>()
            .find(|m| {
                m.provider_type_id() == TypeId::of::<NotificationsTasks>()
                    && m.method() == "purge_expired"
            })
            .expect("NotificationsTasks::purge_expired is discovered");
        assert_eq!(purge.replicas(), Replicas::One);
        assert!(matches!(
            purge.trigger(),
            Trigger::Interval(period) if period == Duration::from_secs(3600)
        ));
    }
}

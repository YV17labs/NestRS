use std::sync::Arc;

use anyhow::Result;
use nest_rs::core::injectable;
use nest_rs::schedule::{CronExpression, scheduled};

use crate::audio::{AUDIO_QUEUE, AudioConfig, AudioService};

#[injectable]
pub struct AudioTasks {
    #[inject]
    svc: Arc<AudioService>,
    #[inject]
    config: Arc<AudioConfig>,
}

#[scheduled]
impl AudioTasks {
    #[every("5s", replicas = "one")]
    async fn enqueue_transcode(&self) -> Result<()> {
        if !self.config.seeds() {
            return Ok(());
        }
        self.svc.seed_and_enqueue().await?;
        Ok(())
    }

    #[after("3s")]
    async fn warmup_on_boot(&self) -> Result<()> {
        tracing::info!(
            target: crate::audio::TARGET,
            phase = "warmup",
            "audio pipeline ready to enqueue",
        );
        Ok(())
    }

    #[cron(CronExpression::EVERY_MINUTE, replicas = "each")]
    async fn heartbeat(&self) -> Result<()> {
        tracing::info!(
            target: crate::audio::TARGET,
            queue = AUDIO_QUEUE,
            "audio producer heartbeat",
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs::schedule::{Replicas, ScheduledMethod};

    use super::AudioTasks;

    #[test]
    fn the_transcode_seed_fires_once_per_occurrence_and_the_heartbeat_on_each_replica() {
        let replicas = |method: &str| {
            nest_rs::core::inventory::iter::<ScheduledMethod>()
                .find(|m| {
                    m.provider_type_id() == TypeId::of::<AudioTasks>() && m.method() == method
                })
                .map(|m| m.replicas())
        };
        assert_eq!(replicas("enqueue_transcode"), Some(Replicas::One));
        assert_eq!(replicas("heartbeat"), Some(Replicas::Each));
        assert_eq!(replicas("warmup_on_boot"), Some(Replicas::Each));
    }
}

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use nestrs_queue::QueueConnection;
use nestrs_schedule::{async_trait, cron_job, CronExpression, Scheduled};

use crate::audio::core::{TranscodeJob, AUDIO_QUEUE};

/// Producer side: a recurring schedule that enqueues jobs the `worker` app
/// consumes. Lives with the producing app (`api`), not the worker, so the
/// worker stays a pure consumer. Shares the `core` contract.
#[cron_job(cron = CronExpression::EVERY_5_SECONDS)]
pub struct AudioProducer {
    #[inject]
    queue: Arc<QueueConnection>,
}

#[async_trait]
impl Scheduled for AudioProducer {
    async fn run(&self) -> Result<()> {
        let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        let file = format!("track-{id}.mp3");
        self.queue
            .of::<TranscodeJob>(AUDIO_QUEUE)
            .push(TranscodeJob { file: file.clone() })
            .await?;
        tracing::info!(target: "features::audio", %file, "scheduled transcode job");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nestrs_core::{Container, Discoverable, DiscoveryService, Module};
    use nestrs_queue::QueueConnection;
    use nestrs_schedule::CronJobMeta;

    use super::AudioProducer;
    use crate::audio::schedule::AudioScheduleModule;

    #[test]
    fn producer_is_discovered_as_a_cron_job() {
        let container = AudioScheduleModule::register(Container::builder()).build();
        let jobs = DiscoveryService::new(&container).meta::<CronJobMeta>();
        assert!(
            jobs.iter().any(|d| d.meta.name == "AudioProducer"),
            "AudioProducer is discovered via #[cron_job]",
        );
    }

    #[test]
    fn producer_declares_its_injected_dependency_for_the_access_graph() {
        assert!(AudioProducer::dependencies().is_empty());
        assert!(
            AudioProducer::injected().contains(&TypeId::of::<QueueConnection>()),
            "the cron job's injected QueueConnection is recorded for the access graph",
        );
    }
}

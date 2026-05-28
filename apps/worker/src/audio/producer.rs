use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use nestrs_queue::QueueConnection;
use nestrs_schedule::{async_trait, cron_job, CronExpression, Scheduled};

use crate::audio::dto::{TranscodeJob, AUDIO_QUEUE};

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
        tracing::info!(target: "worker::audio", %file, "queued transcode job");
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
    use crate::audio::AudioModule;

    #[test]
    fn producer_is_discovered_as_a_cron_job() {
        let container = AudioModule::register(Container::builder()).build();
        let jobs = DiscoveryService::new(&container).meta::<CronJobMeta>();
        assert!(
            jobs.iter().any(|d| d.meta.name == "AudioProducer"),
            "AudioProducer is discovered via #[cron_job]",
        );
    }

    #[test]
    fn producer_declares_its_injected_dependency_for_the_access_graph() {
        // A cron job is built by the Scheduler transport, so its `dependencies`
        // (register ordering) is empty — but `injected` must still report the
        // QueueConnection it pulls, so the access-graph check governs it.
        // Before the `injected`/`dependencies` split this was dropped.
        assert!(AudioProducer::dependencies().is_empty());
        assert!(
            AudioProducer::injected().contains(&TypeId::of::<QueueConnection>()),
            "the cron job's injected QueueConnection is recorded for the access graph",
        );
    }
}

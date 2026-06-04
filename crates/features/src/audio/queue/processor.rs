use std::sync::Arc;

use anyhow::Result;
use nestrs_queue::{async_trait, processor, Processor};

use crate::audio::core::{Transcoder, TranscodeJob};

/// Consumer side: the `worker` app mounts this and processes jobs the `api`
/// app pushed onto the shared `audio` queue.
#[processor(queue = "audio", concurrency = 5, retries = 3)]
pub struct AudioProcessor {
    #[inject]
    transcoder: Arc<Transcoder>,
}

#[async_trait]
impl Processor for AudioProcessor {
    type Job = TranscodeJob;

    async fn process(&self, job: TranscodeJob) -> Result<()> {
        self.transcoder.transcode(&job.file).await
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nestrs_core::{Container, Discoverable, DiscoveryService, Module};
    use nestrs_queue::ProcessorMeta;

    use super::AudioProcessor;
    use crate::audio::core::{Transcoder, AUDIO_QUEUE};
    use crate::audio::queue::AudioQueueModule;

    #[test]
    fn processor_is_discovered_with_its_queue_config() {
        let container = AudioQueueModule::register(Container::builder()).build();
        let processors = DiscoveryService::new(&container).meta::<ProcessorMeta>();
        let audio = processors
            .iter()
            .find(|d| d.meta.name == "AudioProcessor")
            .expect("AudioProcessor is discovered via #[processor]");
        assert_eq!(audio.meta.queue, AUDIO_QUEUE);
        assert_eq!(audio.meta.concurrency, 5);
        assert_eq!(audio.meta.retries, 3);
    }

    #[test]
    fn processor_declares_its_injected_dependency_for_the_access_graph() {
        assert!(AudioProcessor::dependencies().is_empty());
        assert!(
            AudioProcessor::injected().contains(&TypeId::of::<Transcoder>()),
            "the processor's injected Transcoder is recorded for the access graph",
        );
    }
}

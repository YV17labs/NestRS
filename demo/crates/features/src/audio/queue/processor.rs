use std::sync::Arc;

use anyhow::Result;
use nest_rs::core::injectable;
use nest_rs::queue::processor;

use crate::audio::{AudioQueue, AudioService, TranscodeCommand};

#[injectable]
pub struct AudioProcessor {
    #[inject]
    svc: Arc<AudioService>,
}

#[processor]
impl AudioProcessor {
    #[process(queue = AudioQueue, retries = 3, concurrency = 4)]
    async fn transcode(&self, job: TranscodeCommand) -> Result<()> {
        self.svc.transcode(&job.file).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs::core::Discoverable;
    use nest_rs::queue::ProcessMethod;

    use super::AudioProcessor;
    use crate::audio::{AUDIO_QUEUE, AudioService};

    #[test]
    fn process_method_is_discovered_through_the_inventory() {
        let transcode = nest_rs::core::inventory::iter::<ProcessMethod>()
            .find(|m| m.name() == "AudioProcessor::transcode")
            .expect("AudioProcessor::transcode is discovered");
        assert_eq!(transcode.queue(), AUDIO_QUEUE);
        assert_eq!(transcode.options().retries(), 3);
        assert_eq!(transcode.options().concurrency().get(), 4);
    }

    #[test]
    fn injected_dependency_is_recorded_for_the_access_graph() {
        assert!(AudioProcessor::dependencies().contains(&TypeId::of::<AudioService>()));
        assert!(AudioProcessor::injected().contains(&TypeId::of::<AudioService>()));
    }
}

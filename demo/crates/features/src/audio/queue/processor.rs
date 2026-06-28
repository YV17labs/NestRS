use std::sync::Arc;

use anyhow::Result;
use nest_rs_core::injectable;
use nest_rs_queue::processor;

use crate::audio::{AudioService, TranscodeCommand};

#[injectable]
pub struct AudioProcessor {
    #[inject]
    svc: Arc<AudioService>,
}

#[processor]
impl AudioProcessor {
    #[process(queue = "audio", concurrency = 5, retries = 3)]
    async fn transcode(&self, job: TranscodeCommand) -> Result<()> {
        self.svc.transcode(&job.file).await
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_core::Discoverable;
    use nest_rs_queue::ProcessMethod;

    use super::AudioProcessor;
    use crate::audio::{AUDIO_QUEUE, AudioService};

    #[test]
    fn process_method_is_discovered_through_the_inventory() {
        let entries: Vec<&ProcessMethod> = nest_rs_core::inventory::iter::<ProcessMethod>()
            .filter(|m| (m.provider_type_id)() == TypeId::of::<AudioProcessor>())
            .collect();
        let transcode = entries
            .iter()
            .find(|e| e.name == "AudioProcessor::transcode")
            .expect("AudioProcessor::transcode is discovered");
        assert_eq!(transcode.queue, AUDIO_QUEUE);
        assert_eq!(transcode.concurrency, 5);
        assert_eq!(transcode.retries, 3);
    }

    #[test]
    fn injected_dependency_is_recorded_for_the_access_graph() {
        assert!(AudioProcessor::dependencies().contains(&TypeId::of::<AudioService>()));
        assert!(AudioProcessor::injected().contains(&TypeId::of::<AudioService>()));
    }
}

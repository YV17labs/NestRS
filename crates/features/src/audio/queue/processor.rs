use std::sync::Arc;

use anyhow::Result;
use nest_rs_core::injectable;
use nest_rs_queue::processor;

use crate::audio::{AudioService, TranscodeJob};

#[injectable]
pub struct AudioJobs {
    #[inject]
    svc: Arc<AudioService>,
}

#[processor]
impl AudioJobs {
    #[process(queue = "audio", concurrency = 5, retries = 3)]
    async fn transcode(&self, job: TranscodeJob) -> Result<()> {
        self.svc.transcode(&job.file).await
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_core::Discoverable;
    use nest_rs_queue::ProcessMethod;

    use super::AudioJobs;
    use crate::audio::{AUDIO_QUEUE, AudioService};

    #[test]
    fn process_method_is_discovered_through_the_inventory() {
        let entries: Vec<&ProcessMethod> = nest_rs_core::inventory::iter::<ProcessMethod>()
            .filter(|m| (m.provider_type_id)() == TypeId::of::<AudioJobs>())
            .collect();
        let transcode = entries
            .iter()
            .find(|e| e.name == "AudioJobs::transcode")
            .expect("AudioJobs::transcode is discovered");
        assert_eq!(transcode.queue, AUDIO_QUEUE);
        assert_eq!(transcode.concurrency, 5);
        assert_eq!(transcode.retries, 3);
    }

    #[test]
    fn injected_dependency_is_recorded_for_the_access_graph() {
        assert!(AudioJobs::dependencies().contains(&TypeId::of::<AudioService>()));
        assert!(AudioJobs::injected().contains(&TypeId::of::<AudioService>()));
    }
}

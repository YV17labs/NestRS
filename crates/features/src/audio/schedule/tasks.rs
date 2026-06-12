use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use nest_rs_core::injectable;
use nest_rs_schedule::{CronExpression, scheduled};

use crate::audio::{AUDIO_QUEUE, AudioService};

#[injectable]
pub struct AudioTasks {
    #[inject]
    svc: Arc<AudioService>,
}

#[scheduled]
impl AudioTasks {
    #[every("5s")]
    async fn enqueue_transcode(&self) -> Result<()> {
        let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        self.svc.enqueue_transcode(format!("track-{id}.mp3")).await
    }

    #[after("3s")]
    async fn warmup_on_boot(&self) -> Result<()> {
        tracing::info!(
            target: "features::audio",
            phase = "warmup",
            "audio pipeline ready to enqueue",
        );
        Ok(())
    }

    #[cron(CronExpression::EVERY_MINUTE)]
    async fn heartbeat(&self) -> Result<()> {
        tracing::info!(
            target: "features::audio",
            queue = AUDIO_QUEUE,
            "audio producer heartbeat",
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_core::{Discoverable, ReachableProviders};
    use nest_rs_schedule::ScheduledMethod;

    use super::AudioTasks;
    use crate::audio::AudioService;

    #[test]
    fn three_methods_are_discovered_through_the_inventory() {
        let names: Vec<(&'static str, &'static str)> =
            nest_rs_core::inventory::iter::<ScheduledMethod>()
                .filter(|m| (m.provider_type_id)() == TypeId::of::<AudioTasks>())
                .map(|m| (m.provider, m.method))
                .collect();
        assert!(
            names.contains(&("AudioTasks", "enqueue_transcode")),
            "{names:?}"
        );
        assert!(names.contains(&("AudioTasks", "warmup_on_boot")), "{names:?}");
        assert!(names.contains(&("AudioTasks", "heartbeat")), "{names:?}");
    }

    #[test]
    fn injected_dependency_is_recorded_for_the_access_graph() {
        assert!(AudioTasks::dependencies().contains(&TypeId::of::<AudioService>()));
        assert!(AudioTasks::injected().contains(&TypeId::of::<AudioService>()));
    }

    #[test]
    fn reachable_providers_marker_is_a_normal_provider() {
        let _ = TypeId::of::<ReachableProviders>();
    }
}

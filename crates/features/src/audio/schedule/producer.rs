use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use nestrs_core::injectable;
use nestrs_redis::QueueConnection;
use nestrs_schedule::{CronExpression, scheduled};

use crate::audio::core::{AUDIO_QUEUE, TranscodeJob};

/// Producer side: a recurring schedule that enqueues jobs the `worker` app
/// consumes. Lives with the producing app (`api`), not the worker, so the
/// worker stays a pure consumer. Shares the `core` contract.
///
/// Three triggers on one provider — the NestJS-style pattern of pooling
/// related cron methods on a single service so they share `#[inject]`s
/// (here a single [`QueueConnection`]).
#[injectable]
pub struct AudioTasks {
    #[inject]
    queue: Arc<QueueConnection>,
}

#[scheduled]
impl AudioTasks {
    #[every("5s")]
    async fn enqueue_transcode(&self) -> Result<()> {
        let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        let file = format!("track-{id}.mp3");
        self.queue
            .of::<TranscodeJob>(AUDIO_QUEUE)
            .push(TranscodeJob { file: file.clone() })
            .await?;
        tracing::info!(target: "features::audio", %file, "scheduled transcode job");
        Ok(())
    }

    #[after("3s")]
    async fn warmup_on_boot(&self) -> Result<()> {
        tracing::info!(
            target: "features::audio",
            "audio producer warmup — pipeline is ready to enqueue",
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

    use nestrs_core::{Discoverable, ReachableProviders};
    use nestrs_redis::QueueConnection;
    use nestrs_schedule::ScheduledMethod;

    use super::AudioTasks;

    #[test]
    fn three_methods_are_discovered_through_the_inventory() {
        let names: Vec<&'static str> = nestrs_core::inventory::iter::<ScheduledMethod>()
            .filter(|m| (m.provider_type_id)() == TypeId::of::<AudioTasks>())
            .map(|m| m.name)
            .collect();
        assert!(
            names.contains(&"AudioTasks::enqueue_transcode"),
            "{names:?}"
        );
        assert!(names.contains(&"AudioTasks::warmup_on_boot"), "{names:?}");
        assert!(names.contains(&"AudioTasks::heartbeat"), "{names:?}");
    }

    #[test]
    fn injected_dependency_is_recorded_for_the_access_graph() {
        // The provider's own `#[injectable]` emits Discoverable; `#[scheduled]`
        // only adds inventory entries. `dependencies()` records the eagerly
        // required deps the register-phase fixpoint orders against, and
        // `injected()` records the same set for the access-graph check.
        assert!(AudioTasks::dependencies().contains(&TypeId::of::<QueueConnection>()));
        assert!(AudioTasks::injected().contains(&TypeId::of::<QueueConnection>()));
    }

    #[test]
    fn reachable_providers_marker_is_a_normal_provider() {
        // Sanity: ReachableProviders is a concrete type that gets seeded at
        // boot; the scheduler keys its filter on this exact type.
        let _ = TypeId::of::<ReachableProviders>();
    }
}

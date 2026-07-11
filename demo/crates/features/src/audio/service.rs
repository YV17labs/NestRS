use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::injectable;
use nest_rs_queue::{JobProducerExt, QueueError};
use nest_rs_redis::QueueConnection;

use super::command::{AudioQueue, TranscodeCommand};

#[injectable]
pub struct AudioService {
    #[inject]
    queue: Arc<QueueConnection>,
}

impl AudioService {
    /// Producer side: enqueue a transcode job for the worker deployable. The
    /// only failure is the enqueue itself, so it propagates the framework's
    /// [`QueueError`] rather than a feature-local error.
    pub async fn enqueue_transcode(&self, file: String) -> Result<(), QueueError> {
        self.queue
            .push_to::<AudioQueue>(TranscodeCommand { file: file.clone() })
            .await?;
        tracing::debug!(target: "features::audio", file, "enqueued transcode job");
        Ok(())
    }

    /// Consumer side: the work the queue processor delegates to. A simulation
    /// stub with no failure path, so it is honestly infallible — a real
    /// implementation that can fail would introduce its own domain error here.
    pub async fn transcode(&self, file: &str) {
        tokio::time::sleep(Duration::from_millis(300)).await;
        tracing::debug!(target: "features::audio", file, "transcoded");
    }
}

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use nest_rs_core::injectable;
use nest_rs_redis::QueueConnection;

use super::dto::{AUDIO_QUEUE, TranscodeJob};

#[injectable]
pub struct AudioService {
    #[inject]
    queue: Arc<QueueConnection>,
}

impl AudioService {
    pub async fn enqueue_transcode(&self, file: String) -> Result<()> {
        self.queue
            .of::<TranscodeJob>(AUDIO_QUEUE)
            .push(TranscodeJob { file: file.clone() })
            .await?;
        tracing::info!(target: "features::audio", %file, "enqueued transcode job");
        Ok(())
    }

    pub async fn transcode(&self, file: &str) -> Result<()> {
        tokio::time::sleep(Duration::from_millis(300)).await;
        tracing::info!(target: "features::audio", file, "transcoded");
        Ok(())
    }
}

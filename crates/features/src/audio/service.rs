use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use nest_rs_core::injectable;
use nest_rs_redis::QueueConnection;

use super::command::{AUDIO_QUEUE, TranscodeCommand};

#[injectable]
pub struct AudioService {
    #[inject]
    queue: Arc<QueueConnection>,
}

impl AudioService {
    pub async fn enqueue_transcode(&self, file: String) -> Result<()> {
        self.queue
            .of::<TranscodeCommand>(AUDIO_QUEUE)
            .push(TranscodeCommand { file: file.clone() })
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

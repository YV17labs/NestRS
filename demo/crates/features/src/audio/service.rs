use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt, stream};
use nest_rs_core::injectable;
use nest_rs_queue::JobProducerExt;
use nest_rs_redis::QueueConnection;
use nest_rs_storage::Storage;
use uuid::Uuid;

use super::command::{AudioQueue, TranscodeCommand};
use super::error::AudioError;
use super::{PresignedUrlDto, TranscodeEventDto, TranscodeState};

const PRESIGN_TTL: Duration = Duration::from_secs(15 * 60);

const AUDIO_CONTENT_TYPE: &str = "audio/mpeg";

const TRANSCODE_POLL_ATTEMPTS: u32 = 20;
const TRANSCODE_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[injectable]
pub struct AudioService {
    #[inject]
    queue: Arc<QueueConnection>,
    #[inject]
    storage: Arc<Storage>,
}

impl AudioService {
    fn derived_key(source: &str) -> String {
        format!("transcoded/{source}")
    }

    pub async fn presign_upload(&self, filename: &str) -> Result<PresignedUrlDto, AudioError> {
        let key = format!("{}-{filename}", Uuid::now_v7());
        let url = self.storage.presign_put(&key, PRESIGN_TTL).await?;
        tracing::debug!(target: "features::audio", key, "minted presigned upload URL");
        Ok(PresignedUrlDto { key, url })
    }

    pub async fn enqueue_transcode(&self, file: String) -> Result<(), AudioError> {
        self.queue
            .push_to::<AudioQueue>(TranscodeCommand { file: file.clone() })
            .await?;
        tracing::debug!(target: "features::audio", file, "enqueued transcode job");
        Ok(())
    }

    pub async fn seed_and_enqueue(&self, file: String) -> Result<(), AudioError> {
        let bytes = format!("synthetic audio source for {file}").into_bytes();
        self.storage
            .put_bytes(&file, bytes, AUDIO_CONTENT_TYPE)
            .await?;
        self.enqueue_transcode(file).await?;
        Ok(())
    }

    pub async fn transcode(&self, file: &str) -> Result<String, AudioError> {
        let source = self.storage.get_bytes(file).await?;
        let derived = Self::derived_key(file);
        self.storage
            .put_bytes(&derived, source.to_vec(), AUDIO_CONTENT_TYPE)
            .await?;
        tracing::debug!(
            target: "features::audio",
            file,
            derived_key = derived,
            byte_size = source.len(),
            "transcoded",
        );
        Ok(derived)
    }

    pub async fn store_upload(
        &self,
        filename: &str,
        bytes: Vec<u8>,
    ) -> Result<PresignedUrlDto, AudioError> {
        let key = format!("{}-{filename}", Uuid::now_v7());
        self.storage
            .put_bytes(&key, bytes, AUDIO_CONTENT_TYPE)
            .await?;
        let url = self.storage.presign_get(&key, PRESIGN_TTL).await?;
        tracing::debug!(target: "features::audio", key, "stored direct multipart upload");
        Ok(PresignedUrlDto { key, url })
    }

    pub async fn open_result(
        &self,
        file: &str,
    ) -> Result<
        Option<impl Stream<Item = std::io::Result<Bytes>> + Send + 'static + use<>>,
        AudioError,
    > {
        let key = Self::derived_key(file);
        if self.storage.head(&key).await?.is_none() {
            return Ok(None);
        }
        let stream = self
            .storage
            .get_stream(&key)
            .await?
            .map(|chunk| chunk.map_err(std::io::Error::other));
        Ok(Some(stream))
    }

    pub async fn result_ready(&self, file: &str) -> Result<bool, AudioError> {
        let key = Self::derived_key(file);
        Ok(self.storage.head(&key).await?.is_some())
    }

    pub async fn presign_result(&self, file: &str) -> Result<Option<PresignedUrlDto>, AudioError> {
        let key = Self::derived_key(file);
        if self.storage.head(&key).await?.is_none() {
            return Ok(None);
        }
        let url = self.storage.presign_get(&key, PRESIGN_TTL).await?;
        Ok(Some(PresignedUrlDto { key, url }))
    }

    /// Poll the derived object until it exists (or the attempt budget runs
    /// out), yielding one progress event per poll. The transport adapter only
    /// maps each event onto its wire frame.
    pub fn transcode_events(
        self: Arc<Self>,
        file: String,
    ) -> impl Stream<Item = TranscodeEventDto> + Send + 'static {
        stream::unfold(0u32, move |attempt| {
            let svc = self.clone();
            let file = file.clone();
            async move {
                if attempt >= TRANSCODE_POLL_ATTEMPTS {
                    return None;
                }
                let event = |state: TranscodeState| TranscodeEventDto { state, attempt };
                match svc.result_ready(&file).await {
                    Ok(true) => Some((event(TranscodeState::Ready), u32::MAX)),
                    Ok(false) => {
                        tokio::time::sleep(TRANSCODE_POLL_INTERVAL).await;
                        Some((event(TranscodeState::Pending), attempt + 1))
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "features::audio",
                            file = %file,
                            error = %e,
                            "transcode status poll failed",
                        );
                        Some((event(TranscodeState::Error), u32::MAX))
                    }
                }
            }
        })
    }
}

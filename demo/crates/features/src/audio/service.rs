use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt, stream};
use nest_rs::core::{error_message, injectable};
use nest_rs::queue::{JobProducer, JobProducerExt};
use nest_rs::storage::{SignedPut, Storage};
use uuid::Uuid;

use super::command::{AUDIO_QUEUE, AudioQueue, TranscodeCommand};
use super::error::AudioError;
use super::{PresignedUrlDto, TranscodeProgressDto, TranscodeState};

const PRESIGN_TTL: Duration = Duration::from_secs(15 * 60);

const AUDIO_CONTENT_TYPE: &str = "audio/mpeg";

const TRANSCODE_POLL_ATTEMPTS: u32 = 20;
const TRANSCODE_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[injectable]
pub struct AudioService {
    #[inject]
    queue: Arc<dyn JobProducer>,
    #[inject]
    storage: Arc<Storage>,
}

impl AudioService {
    fn derived_key(source: Uuid) -> String {
        format!("transcoded/{source}")
    }

    pub async fn presign_upload(&self) -> Result<PresignedUrlDto, AudioError> {
        let key = Uuid::now_v7().to_string();
        let upload = self
            .storage
            .presign_put(&key, &SignedPut::new(AUDIO_CONTENT_TYPE), PRESIGN_TTL)
            .await?;
        tracing::debug!(target: crate::audio::TARGET, %key, "minted presigned upload URL");
        Ok(PresignedUrlDto::signed(key, upload))
    }

    pub async fn enqueue_transcode(&self, file: Uuid) -> Result<(), AudioError> {
        self.queue
            .push(AudioQueue, TranscodeCommand { file }, None)
            .await?;
        tracing::debug!(target: crate::audio::TARGET, queue = AUDIO_QUEUE, "enqueued transcode job");
        Ok(())
    }

    pub async fn seed_and_enqueue(&self) -> Result<(), AudioError> {
        let file = Uuid::now_v7();
        let bytes = format!("synthetic audio source for {file}").into_bytes();
        self.storage
            .put_bytes(&file.to_string(), bytes, AUDIO_CONTENT_TYPE)
            .await?;
        self.enqueue_transcode(file).await?;
        Ok(())
    }

    pub async fn transcode(&self, file: Uuid) -> Result<String, AudioError> {
        let derived = Self::derived_key(file);
        let source = self
            .storage
            .get_stream(&file.to_string())
            .await?
            .map(|chunk| chunk.map_err(std::io::Error::other));
        self.storage
            .put_stream(&derived, AUDIO_CONTENT_TYPE, source)
            .await?;
        tracing::debug!(
            target: crate::audio::TARGET,
            %file,
            derived_key = derived,
            "transcoded",
        );
        Ok(derived)
    }

    pub async fn store_upload(
        &self,
        part: impl Stream<Item = std::io::Result<Bytes>> + Send,
    ) -> Result<PresignedUrlDto, AudioError> {
        let key = Uuid::now_v7().to_string();
        self.storage
            .put_stream(&key, AUDIO_CONTENT_TYPE, part)
            .await?;
        let download = self.storage.presign_get(&key, PRESIGN_TTL).await?;
        tracing::debug!(target: crate::audio::TARGET, key, "stored direct multipart upload");
        Ok(PresignedUrlDto::signed(key, download))
    }

    pub async fn open_result(
        &self,
        file: Uuid,
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

    pub async fn result_ready(&self, file: Uuid) -> Result<bool, AudioError> {
        let key = Self::derived_key(file);
        Ok(self.storage.head(&key).await?.is_some())
    }

    pub async fn presign_result(&self, file: Uuid) -> Result<Option<PresignedUrlDto>, AudioError> {
        let key = Self::derived_key(file);
        if self.storage.head(&key).await?.is_none() {
            return Ok(None);
        }
        let download = self.storage.presign_get(&key, PRESIGN_TTL).await?;
        Ok(Some(PresignedUrlDto::signed(key, download)))
    }

    pub fn transcode_events(
        self: Arc<Self>,
        file: Uuid,
    ) -> impl Stream<Item = TranscodeProgressDto> + Send + 'static {
        stream::unfold(0u32, move |attempt| {
            let svc = self.clone();
            async move {
                if attempt >= TRANSCODE_POLL_ATTEMPTS {
                    return None;
                }
                let event = |state: TranscodeState| TranscodeProgressDto { state, attempt };
                match svc.result_ready(file).await {
                    Ok(true) => Some((event(TranscodeState::Ready), u32::MAX)),
                    Ok(false) => {
                        tokio::time::sleep(TRANSCODE_POLL_INTERVAL).await;
                        Some((event(TranscodeState::Pending), attempt + 1))
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: crate::audio::TARGET,
                            error = %error_message(&e),
                            "transcode status poll failed",
                        );
                        Some((event(TranscodeState::Error), u32::MAX))
                    }
                }
            }
        })
    }
}

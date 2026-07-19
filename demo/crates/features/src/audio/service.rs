use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use nest_rs_core::injectable;
use nest_rs_queue::{JobProducerExt, QueueError};
use nest_rs_redis::QueueConnection;
use nest_rs_storage::Storage;
use uuid::Uuid;

use super::command::{AudioQueue, TranscodeCommand};
use super::dto::PresignedUrlDto;

const PRESIGN_TTL: Duration = Duration::from_secs(15 * 60);

const AUDIO_CONTENT_TYPE: &str = "audio/mpeg";

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

    pub async fn presign_upload(&self, filename: &str) -> Result<PresignedUrlDto> {
        let key = format!("{}-{filename}", Uuid::now_v7());
        let url = self.storage.presign_put(&key, PRESIGN_TTL).await?;
        tracing::debug!(target: "features::audio", key, "minted presigned upload URL");
        Ok(PresignedUrlDto { key, url })
    }

    pub async fn enqueue_transcode(&self, file: String) -> Result<(), QueueError> {
        self.queue
            .push_to::<AudioQueue>(TranscodeCommand { file: file.clone() })
            .await?;
        tracing::debug!(target: "features::audio", file, "enqueued transcode job");
        Ok(())
    }

    pub async fn seed_and_enqueue(&self, file: String) -> Result<()> {
        let bytes = format!("synthetic audio source for {file}").into_bytes();
        self.storage
            .put_bytes(&file, bytes, AUDIO_CONTENT_TYPE)
            .await?;
        self.enqueue_transcode(file).await?;
        Ok(())
    }

    pub async fn transcode(&self, file: &str) -> Result<String> {
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

    pub async fn store_upload(&self, filename: &str, bytes: Vec<u8>) -> Result<PresignedUrlDto> {
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
    ) -> Result<Option<impl Stream<Item = std::io::Result<Bytes>> + Send + 'static + use<>>> {
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

    pub async fn result_ready(&self, file: &str) -> Result<bool> {
        let key = Self::derived_key(file);
        Ok(self.storage.head(&key).await?.is_some())
    }

    pub async fn presign_result(&self, file: &str) -> Result<Option<PresignedUrlDto>> {
        let key = Self::derived_key(file);
        if self.storage.head(&key).await?.is_none() {
            return Ok(None);
        }
        let url = self.storage.presign_get(&key, PRESIGN_TTL).await?;
        Ok(Some(PresignedUrlDto { key, url }))
    }
}

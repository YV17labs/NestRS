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

/// Validity window for the presigned upload/download URLs the client uses to
/// talk to object storage directly.
const PRESIGN_TTL: Duration = Duration::from_secs(15 * 60);

/// Content type stamped on the objects this pipeline stores.
const AUDIO_CONTENT_TYPE: &str = "audio/mpeg";

#[injectable]
pub struct AudioService {
    #[inject]
    queue: Arc<QueueConnection>,
    #[inject]
    storage: Arc<Storage>,
}

impl AudioService {
    /// Deterministic key of the derived object for a given source key. Keeping it
    /// a pure function of the source is what lets a later read reconstruct it
    /// without recording anything in a database.
    fn derived_key(source: &str) -> String {
        format!("transcoded/{source}")
    }

    /// Mint a presigned `PUT` URL the client uploads bytes to directly — the
    /// server never proxies the payload. The object key is uuid-prefixed so two
    /// uploads of the same filename never clobber each other, yet stays a bare
    /// filename so the follow-up `POST /audio/transcode` passes the
    /// anti-traversal validator on [`TranscodeDto`](super::dto::TranscodeDto).
    pub async fn presign_upload(&self, filename: &str) -> Result<PresignedUrlDto> {
        let key = format!("{}-{filename}", Uuid::now_v7());
        let url = self.storage.presign_put(&key, PRESIGN_TTL).await?;
        tracing::debug!(target: "features::audio", key, "minted presigned upload URL");
        Ok(PresignedUrlDto { key, url })
    }

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

    /// Seed a small synthetic source object, then enqueue its transcode. Drives
    /// the scheduled demo producer so the live pipeline does real object I/O
    /// end-to-end rather than enqueuing keys that were never uploaded (which the
    /// worker's now-real read would reject).
    pub async fn seed_and_enqueue(&self, file: String) -> Result<()> {
        let bytes = format!("synthetic audio source for {file}").into_bytes();
        self.storage
            .put_bytes(&file, bytes, AUDIO_CONTENT_TYPE)
            .await?;
        self.enqueue_transcode(file).await?;
        Ok(())
    }

    /// Consumer side: read the uploaded source object, write a derived object,
    /// and hand back the derived key. The transform itself is simulated — a
    /// container remux that re-stores the bytes under the derived key — but the
    /// reads and writes are real S3 I/O, so it is honestly fallible.
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

    /// Direct-upload side: store bytes the server received (a multipart part)
    /// and hand back the object key plus a presigned `GET` URL for it. The key
    /// is uuid-prefixed like [`presign_upload`](Self::presign_upload)'s, so the
    /// follow-up transcode/download accept it. Unlike the presigned path the
    /// server proxies the payload — the cost of a single round-trip upload.
    pub async fn store_upload(&self, filename: &str, bytes: Vec<u8>) -> Result<PresignedUrlDto> {
        let key = format!("{}-{filename}", Uuid::now_v7());
        self.storage
            .put_bytes(&key, bytes, AUDIO_CONTENT_TYPE)
            .await?;
        let url = self.storage.presign_get(&key, PRESIGN_TTL).await?;
        tracing::debug!(target: "features::audio", key, "stored direct multipart upload");
        Ok(PresignedUrlDto { key, url })
    }

    /// Streamed read side: open the derived object as a chunked byte stream the
    /// controller feeds straight into a streamed HTTP body — the large-file
    /// download never sits whole in process memory. `None` while the worker has
    /// not produced the derived object yet.
    pub async fn open_result(
        &self,
        file: &str,
    ) -> Result<Option<impl Stream<Item = std::io::Result<Bytes>> + Send + 'static + use<>>> {
        let key = Self::derived_key(file);
        if self.storage.head(&key).await?.is_none() {
            return Ok(None);
        }
        // poem's streamed body wants an `io::Error`; a mid-stream storage read
        // failure surfaces as one rather than silently truncating the download.
        let stream = self
            .storage
            .get_stream(&key)
            .await?
            .map(|chunk| chunk.map_err(std::io::Error::other));
        Ok(Some(stream))
    }

    /// Whether the worker has produced the derived object yet — a cheap `head`,
    /// no presign. Drives the `GET /audio/events` SSE progress feed.
    pub async fn result_ready(&self, file: &str) -> Result<bool> {
        let key = Self::derived_key(file);
        Ok(self.storage.head(&key).await?.is_some())
    }

    /// Read side: if the worker has produced the derived object, mint a presigned
    /// `GET` URL for it; `None` while it does not exist yet.
    pub async fn presign_result(&self, file: &str) -> Result<Option<PresignedUrlDto>> {
        let key = Self::derived_key(file);
        if self.storage.head(&key).await?.is_none() {
            return Ok(None);
        }
        let url = self.storage.presign_get(&key, PRESIGN_TTL).await?;
        Ok(Some(PresignedUrlDto { key, url }))
    }
}

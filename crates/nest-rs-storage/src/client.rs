use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::Method;
use nest_rs_core::injectable;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path;
use object_store::signer::Signer;
use object_store::{
    Attribute, Attributes, MultipartUpload, ObjectStore, ObjectStoreExt, PutMultipartOptions,
    PutOptions, PutPayload,
};

use crate::config::StorageConfig;
use crate::error::{Result, StorageError};

/// Bytes buffered before a multipart part is shipped. S3 requires every part
/// but the last to be at least 5 MiB, so a smaller value would make
/// [`put_stream`](Storage::put_stream) fail on any upload past one part.
///
/// `pub` for the e2e suite alone — a test of the abort path has to build a
/// payload that provably ships a part *before* it fails, and a copied `5 * 1024
/// * 1024` there would keep passing while proving less the day this moves.
/// Hidden because a caller has nothing to do with it: `put_stream` buffers on
/// its own, whatever size the source yields.
#[doc(hidden)]
pub const MULTIPART_PART_SIZE: usize = 5 * 1024 * 1024;

/// Thin, injectable S3-compatible object-store client built lazily from
/// [`StorageConfig`] over the [`object_store`] crate.
///
/// The backing driver is `object_store`'s [`AmazonS3`], which implements both
/// [`ObjectStore`] (byte read/write, head) and [`Signer`] (presigned URLs). It
/// speaks to real AWS S3 as well as any S3-compatible server (MinIO, RustFS) in
/// path- or virtual-host style. Because the seam is the `object_store` traits,
/// swapping to GCS/Azure/local-fs/in-memory later is a one-line builder change,
/// not a rewrite of this type.
///
/// The client is constructed once on first use via [`OnceLock`] so the provider
/// stays cheap to inject and the (synchronous) builder cost is paid lazily.
#[injectable]
pub struct Storage {
    #[inject]
    config: Arc<StorageConfig>,
    store: OnceLock<AmazonS3>,
}

impl Storage {
    /// Construct directly from a config, bypassing the DI container.
    ///
    /// The DI path uses the generated `from_container` constructor; this is the
    /// honest constructor for tests and ad-hoc tooling that hold a
    /// [`StorageConfig`] without standing up a container.
    pub fn new(config: Arc<StorageConfig>) -> Self {
        Self {
            config,
            store: OnceLock::new(),
        }
    }

    /// The S3 driver, built once on first use. Returns [`StorageError::Init`]
    /// instead of panicking when the configured values can't produce a client.
    fn store(&self) -> Result<&AmazonS3> {
        if let Some(store) = self.store.get() {
            return Ok(store);
        }
        // Last line of defence for the plain-HTTP rule. `StorageConfig` already
        // refuses the `http://` + `allow_http = false` pairing at load, so this
        // is unreachable through the DI path; it stays for a hand-built
        // `Storage::new`, where no config resolution ran. Checked here rather
        // than per method because `object_store`'s own `with_allow_http` only
        // gates *transfers* — presigning is a local computation, so a plaintext
        // endpoint would otherwise hand clients a working URL carrying the
        // SigV4 signature.
        if crate::config::is_plaintext(&self.config.endpoint) && !self.config.allow_http {
            return Err(StorageError::PlaintextEndpoint {
                endpoint: self.config.endpoint.clone(),
            });
        }
        let built = AmazonS3Builder::new()
            .with_endpoint(&self.config.endpoint)
            .with_region(&self.config.region)
            .with_access_key_id(&self.config.access_key)
            .with_secret_access_key(&self.config.secret_key)
            .with_bucket_name(&self.config.bucket)
            // Opt-in plain-HTTP (default on in dev/test, off in prod — STORAGE-ST2)
            // so a RustFS/MinIO dev server is reachable while production refuses
            // to send credentials over an unencrypted endpoint by omission.
            .with_allow_http(self.config.allow_http)
            // `force_path_style` ⇒ path-style addressing, i.e. *not*
            // virtual-hosted-style.
            .with_virtual_hosted_style_request(!self.config.force_path_style)
            .build()
            .map_err(StorageError::Init)?;
        // A racing thread may have initialized first — `get_or_init` keeps the
        // winner and drops our `built`; either way one client is shared.
        Ok(self.store.get_or_init(|| built))
    }

    /// The configured bucket every key in this client is addressed within.
    pub fn bucket_name(&self) -> &str {
        &self.config.bucket
    }

    /// Sign a short-lived URL for `method` against `key`. The plain-HTTP rule
    /// is enforced by [`store`](Self::store), which every operation goes
    /// through.
    async fn presigned_url(&self, method: Method, key: &str, expires: Duration) -> Result<String> {
        let label = method.to_string();
        let url = self
            .store()?
            .signed_url(method, &Path::from(key), expires)
            .await
            .map_err(|source| StorageError::Presign {
                method: label,
                source,
            })?;
        Ok(url.to_string())
    }

    /// Presigned `PUT` URL the client uploads bytes to directly.
    ///
    /// Content-type is set by the uploading client on the PUT and read back at
    /// confirm time, so it is intentionally not signed here.
    pub async fn presign_put(&self, key: &str, expires: Duration) -> Result<String> {
        self.presigned_url(Method::PUT, key, expires).await
    }

    /// Presigned `GET` URL — for serving private originals on demand.
    pub async fn presign_get(&self, key: &str, expires: Duration) -> Result<String> {
        self.presigned_url(Method::GET, key, expires).await
    }

    /// Byte size of an uploaded object (used to finalize a record). Returns
    /// `None` if the object does not exist yet.
    ///
    /// NOTE: `object_store`'s [`ObjectMeta`](object_store::ObjectMeta) does not
    /// carry the stored `Content-Type`, so it is not returned here. Callers that
    /// need the mime type should keep the value they supplied at
    /// upload-request time rather than relying on `head`.
    pub async fn head(&self, key: &str) -> Result<Option<HeadMetadata>> {
        match self.store()?.head(&Path::from(key)).await {
            Ok(meta) => Ok(Some(HeadMetadata {
                byte_size: meta.size as i64,
            })),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(StorageError::Head(e)),
        }
    }

    /// Download an object's full bytes (e.g. a media worker reads the original).
    ///
    /// Returns `object_store`'s `Bytes` directly — an `Arc`-backed buffer that
    /// clones cheaply — so the body is never copied on the way out.
    pub async fn get_bytes(&self, key: &str) -> Result<Bytes> {
        let result = self
            .store()?
            .get(&Path::from(key))
            .await
            .map_err(StorageError::Get)?;
        result.bytes().await.map_err(StorageError::Get)
    }

    /// Stream an object's bytes chunk by chunk instead of buffering the whole
    /// body ([`get_bytes`](Self::get_bytes) collects; this does not).
    ///
    /// The returned stream drives the S3 `GetObject` response directly, so a
    /// large media file flows to the client without ever sitting whole in
    /// process memory — feed it to a streaming HTTP body. Each item is a
    /// [`Result`] so a mid-stream transport error still surfaces rather than
    /// silently truncating.
    pub async fn get_stream(
        &self,
        key: &str,
    ) -> Result<impl futures_util::Stream<Item = Result<Bytes>> + Send + 'static + use<>> {
        use futures_util::StreamExt;
        let result = self
            .store()?
            .get(&Path::from(key))
            .await
            .map_err(StorageError::Get)?;
        Ok(result
            .into_stream()
            .map(|chunk| chunk.map_err(StorageError::Get)))
    }

    /// List the objects stored under `prefix`, one entry at a time.
    ///
    /// Streamed rather than collected for the same reason as
    /// [`get_stream`](Self::get_stream): a bucket can hold far more keys than a
    /// `Vec` should ever hold, and S3 pages the listing anyway — the returned
    /// stream fetches the next page as it is consumed. Pass `""` for the whole
    /// bucket.
    ///
    /// `prefix` matches on **path segments**, not on characters: `posts/cover`
    /// is a prefix of `posts/cover/a.png` but not of `posts/cover-2.png`. The
    /// match is recursive, so nested keys are included.
    pub fn list(
        &self,
        prefix: &str,
    ) -> Result<impl futures_util::Stream<Item = Result<ObjectEntry>> + Send + 'static + use<>>
    {
        use futures_util::StreamExt;
        Ok(self
            .store()?
            .list(Some(&Path::from(prefix)))
            .map(|meta| match meta {
                Ok(meta) => Ok(ObjectEntry {
                    key: meta.location.as_ref().to_string(),
                    byte_size: meta.size as i64,
                    last_modified: meta.last_modified.into(),
                }),
                Err(e) => Err(StorageError::List(e)),
            }))
    }

    /// Upload bytes (e.g. a media worker writes a WebP variant).
    ///
    /// Takes anything convertible to [`Bytes`], so a `Vec<u8>` and the `Bytes`
    /// [`get_bytes`](Self::get_bytes) hands back both compose without a copy —
    /// the read/write round-trip the storage docs show is one expression.
    pub async fn put_bytes(
        &self,
        key: &str,
        bytes: impl Into<Bytes> + Send,
        content_type: &str,
    ) -> Result<()> {
        let mut attributes = Attributes::new();
        attributes.insert(Attribute::ContentType, content_type.to_string().into());
        let opts = PutOptions {
            attributes,
            ..Default::default()
        };
        self.store()?
            .put_opts(&Path::from(key), bytes.into().into(), opts)
            .await
            .map_err(StorageError::Put)?;
        Ok(())
    }

    /// Upload an object from a byte stream, without ever holding it whole.
    ///
    /// The counterpart to [`put_bytes`](Self::put_bytes) for a body whose size
    /// is unknown or larger than memory — an inbound HTTP upload, a file read
    /// chunk by chunk. Bytes are buffered into 5 MiB multipart parts, so peak
    /// memory is one part regardless of the object's size, and the object
    /// becomes visible only once every part landed.
    ///
    /// The source yields [`std::io::Result`] because that is what byte streams
    /// in this ecosystem speak (poem bodies, `tokio::io`); a
    /// [`StorageError`] converts into `std::io::Error`, so a stream read out of
    /// storage can be written straight back into it.
    ///
    /// Any failure — the store's or the source's — aborts the upload before
    /// returning, because S3 bills the parts of an interrupted multipart upload
    /// until something removes them. So does **not returning at all**: a request
    /// timeout or a client disconnect drops this future mid-part, and
    /// [`UploadGuard`] is what turns that into an abort rather than into parts
    /// nobody will ever collect.
    pub async fn put_stream<S>(&self, key: &str, content_type: &str, stream: S) -> Result<()>
    where
        S: futures_util::Stream<Item = std::io::Result<Bytes>> + Send,
    {
        use futures_util::StreamExt;

        let mut attributes = Attributes::new();
        attributes.insert(Attribute::ContentType, content_type.to_string().into());
        let opts = PutMultipartOptions {
            attributes,
            ..Default::default()
        };
        let mut upload = UploadGuard::new(
            self.store()?
                .put_multipart_opts(&Path::from(key), opts)
                .await
                .map_err(StorageError::Put)?,
            key,
        );

        let mut stream = std::pin::pin!(stream);
        let mut pending: Vec<Bytes> = Vec::new();
        let mut pending_len = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(source) => {
                    upload.abort().await;
                    return Err(StorageError::PutSource(source));
                }
            };
            pending_len += chunk.len();
            pending.push(chunk);
            if pending_len < MULTIPART_PART_SIZE {
                continue;
            }
            // The chunks are shipped as they are rather than re-split to an
            // exact size: `PutPayload` is a list of `Bytes`, so a part costs no
            // copy, and S3 only bounds a part from below.
            if let Err(e) = upload
                .get()
                .put_part(PutPayload::from_iter(pending.drain(..)))
                .await
            {
                upload.abort().await;
                return Err(StorageError::Put(e));
            }
            pending_len = 0;
        }

        // The tail ships even when empty: a multipart upload with no part at all
        // is rejected on completion, so a zero-byte stream still needs one.
        if let Err(e) = upload.get().put_part(PutPayload::from_iter(pending)).await {
            upload.abort().await;
            return Err(StorageError::Put(e));
        }
        if let Err(e) = upload.get().complete().await {
            upload.abort().await;
            return Err(StorageError::Put(e));
        }
        upload.finished();
        Ok(())
    }

    /// Delete an object. Absent keys succeed, so retention sweeps and
    /// failed-upload cleanup are idempotent.
    ///
    /// Without this an app had to drop to `object_store` directly to implement
    /// a retention policy or a GDPR erasure — a seam the docs describe as
    /// internal.
    pub async fn delete(&self, key: &str) -> Result<()> {
        match self.store()?.delete(&Path::from(key)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(StorageError::Delete(e)),
        }
    }
}

/// Holds a multipart upload so that **not finishing** is an outcome the store
/// hears about too.
///
/// Every path that returns aborts explicitly, and did before this existed. The
/// path that does not return is cancellation — a request timeout (30s by
/// default), a client that hung up — where the future is simply dropped and no
/// `.await` in it will ever run again. The parts stayed on the store, billed
/// until a lifecycle rule swept them, and nothing was logged at all: it is the
/// likeliest interruption for exactly the uploads streaming exists for.
///
/// `Drop` cannot await, so it hands the abort to a detached task. That is
/// best-effort by construction — the process may be shutting down — which is
/// why it is a `warn` naming the key rather than a silent cleanup: an operator
/// reading a billing surprise needs the key, and a suite needs something to
/// assert on.
struct UploadGuard {
    upload: Option<Box<dyn MultipartUpload>>,
    key: String,
}

impl UploadGuard {
    fn new(upload: Box<dyn MultipartUpload>, key: &str) -> Self {
        Self {
            upload: Some(upload),
            key: key.to_owned(),
        }
    }

    /// The upload itself, for the duration of one call.
    fn get(&mut self) -> &mut Box<dyn MultipartUpload> {
        self.upload
            .as_mut()
            .expect("the upload is taken only by `abort` or `finished`, which both consume it")
    }

    /// Abort now, in order, and disarm — so the explicit failure paths keep
    /// emitting their event synchronously and a test can still assert on it.
    async fn abort(&mut self) {
        if let Some(mut upload) = self.upload.take() {
            abort_upload(&mut upload, &self.key).await;
        }
    }

    /// The upload completed; there is nothing left to discard.
    fn finished(&mut self) {
        self.upload = None;
    }
}

impl Drop for UploadGuard {
    fn drop(&mut self) {
        let Some(mut upload) = self.upload.take() else {
            return;
        };
        let key = std::mem::take(&mut self.key);
        tracing::warn!(
            target: "nest_rs::storage",
            key = key.as_str(),
            "multipart upload was cancelled mid-flight; discarding its parts",
        );
        // Only reachable from inside the runtime the upload was driven by, but
        // a `Drop` has no way to prove that — and panicking in a destructor
        // while unwinding a cancellation would replace a billing leak with a
        // crash.
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move { abort_upload(&mut upload, &key).await });
            }
            Err(_) => tracing::warn!(
                target: "nest_rs::storage",
                key = key.as_str(),
                "no runtime is available to discard them; they are left for the store's \
                 lifecycle rule",
            ),
        }
    }
}

/// Discard the parts of a multipart upload that will never complete.
///
/// Best-effort by necessity: the failure that brought us here is what the
/// caller needs back, so a failing abort can only be reported. It is reported
/// loudly — orphaned parts are invisible to a `list` and billed until a
/// lifecycle rule sweeps them.
///
/// The success is reported too, and that is not symmetry for its own sake: an
/// interrupted upload materializes no object either way, so **whether the parts
/// were discarded is not observable through the store's API at all** — S3
/// answers it only through `ListMultipartUploads`, which `object_store` does not
/// surface. This event is what an operator reads under a billing surprise, and
/// what the e2e suite asserts on to keep the abort from being refactored away
/// silently.
async fn abort_upload(upload: &mut Box<dyn MultipartUpload>, key: &str) {
    match upload.abort().await {
        Ok(()) => tracing::debug!(
            target: "nest_rs::storage",
            key,
            "discarded the parts of an interrupted multipart upload",
        ),
        Err(error) => tracing::warn!(
            target: "nest_rs::storage",
            key,
            error = %error,
            "multipart upload left dangling parts",
        ),
    }
}

/// Result of a `head` — the metadata we cache onto a stored-file record.
pub struct HeadMetadata {
    /// The object's size in bytes, as reported by S3.
    pub byte_size: i64,
}

/// One object yielded by [`Storage::list`].
///
/// Deliberately built from `std` types alone: `object_store` reports a
/// timestamp as a `chrono::DateTime<Utc>`, and re-exporting that would make
/// every consumer of this crate pin the same `chrono` major to read a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectEntry {
    /// The object's key, as addressed within the bucket.
    pub key: String,
    /// The object's size in bytes, as reported by S3.
    pub byte_size: i64,
    /// When the object was last written.
    pub last_modified: SystemTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(endpoint: &str, allow_http: bool) -> Storage {
        Storage::new(Arc::new(StorageConfig {
            endpoint: endpoint.into(),
            allow_http,
            ..Default::default()
        }))
    }

    // G1: presigning is a local computation, so `object_store`'s allow_http
    // never saw it — production minted working `http://` URLs carrying the
    // SigV4 signature. `StorageConfig` now rejects the pairing at boot; this
    // covers the one path that skips config resolution, `Storage::new`.
    #[tokio::test]
    async fn presigning_refuses_a_plaintext_endpoint_when_http_is_disallowed() {
        let storage = client("http://minio.internal:9000", false);
        for signed in [
            storage
                .presign_put("k", Duration::from_secs(900))
                .await
                .err(),
            storage
                .presign_get("k", Duration::from_secs(900))
                .await
                .err(),
        ] {
            let err = signed.expect("a plaintext presigned URL must never be minted");
            assert!(
                matches!(err, StorageError::PlaintextEndpoint { .. }),
                "got {err:?}",
            );
            assert!(err.to_string().contains("NESTRS_STORAGE__ALLOW_HTTP"));
        }
    }

    #[tokio::test]
    async fn presigning_over_an_encrypted_endpoint_is_untouched() {
        // Signing is local, so this needs no server — reaching the signer at
        // all proves the guard did not fire.
        let storage = client("https://s3.example", false);
        storage
            .presign_get("k", Duration::from_secs(900))
            .await
            .expect("https is always allowed");
    }

    // Every operation goes through `store()`, so a method added later inherits
    // the guard — unless it reaches the network before asking for the client.
    // These two never open a socket: both fail while the endpoint is still a
    // string.
    #[tokio::test]
    async fn listing_and_streamed_uploads_refuse_a_plaintext_endpoint_too() {
        let storage = client("http://minio.internal:9000", false);
        let listed = storage.list("").err();
        let uploaded = storage
            .put_stream("k", "text/plain", futures_util::stream::empty())
            .await
            .err();
        for refused in [listed, uploaded] {
            let err = refused.expect("a plaintext endpoint must never be addressed");
            assert!(
                matches!(err, StorageError::PlaintextEndpoint { .. }),
                "got {err:?}",
            );
        }
    }

    // G13: the streaming page shows `Body::from_bytes_stream(stream)` fed
    // straight from `get_stream`, which needs `E: Into<std::io::Error>`.
    #[test]
    fn a_storage_error_converts_into_io_error_so_streams_compose() {
        let io: std::io::Error = StorageError::PlaintextEndpoint {
            endpoint: "http://x".into(),
        }
        .into();
        assert!(io.to_string().contains("plain HTTP"));
    }
}

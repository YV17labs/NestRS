use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use http::Method;
use nest_rs_core::injectable;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path;
use object_store::signer::Signer;
use object_store::{Attribute, Attributes, ObjectStore, ObjectStoreExt, PutOptions};

use crate::config::StorageConfig;
use crate::error::{Result, StorageError};

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

/// Result of a `head` — the metadata we cache onto a stored-file record.
pub struct HeadMetadata {
    /// The object's size in bytes, as reported by S3.
    pub byte_size: i64,
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

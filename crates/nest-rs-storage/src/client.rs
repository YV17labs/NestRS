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
        let built = AmazonS3Builder::new()
            .with_endpoint(&self.config.endpoint)
            .with_region(&self.config.region)
            .with_access_key_id(&self.config.access_key)
            .with_secret_access_key(&self.config.secret_key)
            .with_bucket_name(&self.config.bucket)
            // RustFS/MinIO dev servers are reached over plain `http://`.
            .with_allow_http(true)
            // `force_path_style` ⇒ path-style addressing, i.e. *not*
            // virtual-hosted-style.
            .with_virtual_hosted_style_request(!self.config.force_path_style)
            .build()
            .map_err(StorageError::Init)?;
        // A racing thread may have initialized first — `get_or_init` keeps the
        // winner and drops our `built`; either way one client is shared.
        Ok(self.store.get_or_init(|| built))
    }

    pub fn bucket_name(&self) -> &str {
        &self.config.bucket
    }

    /// Sign a short-lived URL for `method` against `key`.
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

    /// Upload bytes (e.g. a media worker writes a WebP variant).
    pub async fn put_bytes(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<()> {
        let mut attributes = Attributes::new();
        attributes.insert(Attribute::ContentType, content_type.to_string().into());
        let opts = PutOptions {
            attributes,
            ..Default::default()
        };
        self.store()?
            .put_opts(&Path::from(key), bytes.into(), opts)
            .await
            .map_err(StorageError::Put)?;
        Ok(())
    }
}

/// Result of a `head` — the metadata we cache onto a stored-file record.
pub struct HeadMetadata {
    pub byte_size: i64,
}

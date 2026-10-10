use std::any::TypeId;
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::Method;
use nest_rs_config::{ConfigService, Namespaced};
use nest_rs_core::{
    Budget, Container, ContainerBuilder, Discoverable, ProviderResidency, TaskContext,
};
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path;
use object_store::signer::Signer;
use object_store::{
    Attribute, Attributes, BackoffConfig, ClientOptions, MultipartUpload, ObjectStore,
    ObjectStoreExt, PutMultipartOptions, PutOptions, PutPayload, RetryConfig,
};

use crate::config::{OPERATION_TIMEOUT, READ_TIMEOUT, StorageConfig};
use crate::error::{Result, StorageError};
use crate::transfer::download;

/// The longest a dial to S3 waits — DNS, TCP and the TLS handshake; never
/// scaled down with the budget, since a dial cut shorter never connects.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Retries stop at half of `budget` and back off at most an eighth, so a call
/// whose attempts keep failing ends on S3's own error before the budget cuts it.
fn retries_within(budget: Duration) -> RetryConfig {
    let max_backoff = budget / 8;
    RetryConfig {
        backoff: BackoffConfig {
            init_backoff: BackoffConfig::default().init_backoff.min(max_backoff),
            max_backoff,
            ..BackoffConfig::default()
        },
        retry_timeout: budget / 2,
        ..RetryConfig::default()
    }
}

/// Bytes buffered before a multipart part is shipped: S3 requires every part
/// but the last to be at least 5 MiB.
pub const MULTIPART_PART_SIZE: usize = 5 * 1024 * 1024;

/// Thin, injectable S3-compatible object-store client built lazily from
/// [`StorageConfig`] over the [`object_store`] crate.
///
/// Every call waits on S3 within [`StorageConfig::operation_timeout`], and a
/// download, while it is read, for its next bytes within
/// [`StorageConfig::read_timeout`].
pub struct Storage {
    config: Arc<StorageConfig>,
    store: OnceLock<Arc<AmazonS3>>,
}

// Registered by hand rather than by `#[injectable]` for what the decorator
// cannot say: the client's budget, held under every net reaching it.
impl Discoverable for Storage {
    fn dependencies() -> Vec<TypeId> {
        vec![TypeId::of::<StorageConfig>()]
    }

    fn dependency_names() -> Vec<&'static str> {
        vec!["StorageConfig"]
    }

    fn injected() -> Vec<TypeId> {
        Self::dependencies()
    }

    fn injected_names() -> Vec<&'static str> {
        Self::dependency_names()
    }

    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        let storage = Self::from_container(&builder.snapshot());
        builder.provide(storage).provide_meta(Budget::of::<Self>(
            "the object store",
            format!(
                "{}, or `{}` in code",
                nest_rs_config::var_name("storage", OPERATION_TIMEOUT.key()),
                OPERATION_TIMEOUT.field(),
            ),
            |storage| Some(storage.config.operation_timeout),
        ))
    }
}

impl ProviderResidency for Storage {
    const SINGLETON: bool = true;
}

impl Storage {
    /// Construct directly from a config, bypassing the DI container.
    pub fn new(config: Arc<StorageConfig>) -> Self {
        Self {
            config,
            store: OnceLock::new(),
        }
    }

    /// Construct this provider by resolving its config from the container —
    /// what the register phase calls, not by hand.
    pub fn from_container(container: &Container) -> Self {
        #[expect(
            clippy::expect_used,
            reason = "the register phase builds a provider only once its `dependencies` are registered"
        )]
        let config = container
            .get::<StorageConfig>()
            .expect("Storage.config: no provider registered for this dependency");
        Self::new(config)
    }

    /// The S3 driver, built once on first use.
    fn store(&self) -> Result<&Arc<AmazonS3>> {
        if let Some(store) = self.store.get() {
            return Ok(store);
        }
        // For a hand-built `Storage::new`: `object_store`'s `with_allow_http`
        // gates transfers only, so presigning would mint a plaintext signed URL.
        if crate::config::is_plaintext(&self.config.endpoint) && !self.config.allow_http {
            return Err(StorageError::PlaintextEndpoint {
                endpoint: self.config.endpoint.clone(),
            });
        }
        // A config handed to `new` skipped `from_env`, and the ranges with it.
        for (bounds, value) in [
            (OPERATION_TIMEOUT, self.config.operation_timeout),
            (READ_TIMEOUT, self.config.read_timeout),
        ] {
            bounds
                .check("storage", bounds.field(), value)
                .map_err(|source| {
                    StorageError::Init(object_store::Error::Generic {
                        store: "S3",
                        source: Box::new(source),
                    })
                })?;
        }
        // One `ClientOptions` value: `with_client_options` replaces the whole
        // set, so a builder call made before it would be undone.
        let mut options = ClientOptions::new()
            .with_allow_http(self.config.allow_http)
            // `object_store`'s default timeout bounds an attempt's body too,
            // cutting a download for its size; the budget and `download` do.
            .with_timeout_disabled()
            .with_connect_timeout(CONNECT_TIMEOUT);
        // A config handed to `new` skipped `from_env`, and its refusals with it.
        crate::config::refuse_tls(
            &ConfigService::for_namespace(StorageConfig::NAMESPACE),
            &self.config.endpoint,
            &self.config.tls,
        )
        .map_err(|source| {
            StorageError::Init(object_store::Error::Generic {
                store: "S3",
                source: Box::new(source),
            })
        })?;
        // Only the authorities `authorities_pem` hands over — the system's
        // unless the deployment named others — so every client the framework
        // opens trusts through one read of the store.
        let authorities =
            object_store::Certificate::from_pem_bundle(self.config.tls.authorities_pem())
                .map_err(StorageError::Init)?;
        options = authorities.into_iter().fold(
            options.with_no_system_certificates(true),
            ClientOptions::with_root_certificate,
        );
        let built = AmazonS3Builder::new()
            .with_client_options(options)
            .with_retry(retries_within(self.config.operation_timeout))
            .with_endpoint(&self.config.endpoint)
            .with_region(&self.config.region)
            .with_access_key_id(&self.config.access_key)
            .with_secret_access_key(&self.config.secret_key)
            .with_bucket_name(&self.config.bucket)
            .with_virtual_hosted_style_request(!self.config.force_path_style)
            .build()
            .map_err(StorageError::Init)?;
        // A racing thread may have won: `get_or_init` keeps its client.
        Ok(self.store.get_or_init(|| Arc::new(built)))
    }

    /// The configured bucket every key in this client is addressed within.
    pub fn bucket_name(&self) -> &str {
        &self.config.bucket
    }

    /// Sign a short-lived URL for `method` against `key`.
    async fn presigned_url(&self, method: Method, key: &str, expires: Duration) -> Result<String> {
        let label = method.to_string();
        let url = self
            .answered(self.store()?.signed_url(method, &Path::from(key), expires))
            .await
            .map_err(|source| StorageError::Presign {
                method: label,
                source,
            })?;
        Ok(url.to_string())
    }

    /// Presigned `PUT` URL the client uploads bytes to directly; the content
    /// type is not signed, so the uploading client sets it.
    pub async fn presign_put(&self, key: &str, expires: Duration) -> Result<String> {
        self.presigned_url(Method::PUT, key, expires).await
    }

    /// Presigned `GET` URL — for serving private originals on demand.
    pub async fn presign_get(&self, key: &str, expires: Duration) -> Result<String> {
        self.presigned_url(Method::GET, key, expires).await
    }

    /// Byte size of an uploaded object, `None` if it does not exist yet.
    ///
    /// `object_store` does not report the stored `Content-Type`: a caller that
    /// needs it keeps the one it supplied at upload.
    pub async fn head(&self, key: &str) -> Result<Option<ObjectMetadata>> {
        match self.answered(self.store()?.head(&Path::from(key))).await {
            Ok(meta) => Ok(Some(ObjectMetadata {
                byte_size: meta.size as i64,
            })),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(StorageError::Head(e)),
        }
    }

    /// Download an object's full bytes (e.g. a media worker reads the original).
    pub async fn get_bytes(&self, key: &str) -> Result<Bytes> {
        use futures_util::StreamExt;
        let (size, body) = self.transfer(key).await?;
        let mut body = std::pin::pin!(body);
        let Some(first) = body.next().await.transpose()? else {
            return Ok(Bytes::new());
        };
        let Some(second) = body.next().await.transpose()? else {
            return Ok(first);
        };
        let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or_default());
        bytes.extend_from_slice(&first);
        bytes.extend_from_slice(&second);
        while let Some(chunk) = body.next().await {
            bytes.extend_from_slice(&chunk?);
        }
        Ok(Bytes::from(bytes))
    }

    /// Stream an object's bytes chunk by chunk instead of buffering the whole
    /// body.
    pub async fn get_stream(
        &self,
        key: &str,
    ) -> Result<impl futures_util::Stream<Item = Result<Bytes>> + Send + 'static + use<>> {
        Ok(self.transfer(key).await?.1)
    }

    /// The object at `key`'s size, and its bytes as S3 sends them.
    async fn transfer(
        &self,
        key: &str,
    ) -> Result<(
        u64,
        impl futures_util::Stream<Item = Result<Bytes>> + Send + 'static + use<>,
    )> {
        let store = self.store()?;
        let path = Path::from(key);
        let first = self
            .answered(store.get(&path))
            .await
            .map_err(StorageError::Get)?;
        let size = first.range.end - first.range.start;
        Ok((
            size,
            download(
                Arc::clone(store) as Arc<dyn ObjectStore>,
                path,
                first,
                self.config.read_timeout,
                self.config.operation_timeout,
            ),
        ))
    }

    /// List the objects stored under `prefix`, one entry at a time, fetching
    /// the next page as the stream is consumed. Pass `""` for the whole bucket.
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
        let budget = self.config.operation_timeout;
        let pages = self.store()?.list(Some(&Path::from(prefix)));
        let entries = Box::pin(futures_util::stream::unfold(
            Some(pages),
            move |pages| async move {
                let mut pages = pages?;
                let entry = match bounded(budget, pages.next()).await {
                    Ok(None) => return None,
                    Ok(Some(entry)) => entry,
                    Err(unanswered) => return Some((Err(unanswered), None)),
                };
                Some((entry, Some(pages)))
            },
        ));
        Ok(entries.map(|meta| match meta {
            Ok(meta) => Ok(ObjectEntry {
                key: meta.location.as_ref().to_string(),
                byte_size: meta.size as i64,
                last_modified: meta.last_modified.into(),
            }),
            Err(e) => Err(StorageError::List(e)),
        }))
    }

    /// Upload bytes (e.g. a media worker writes a WebP variant).
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
        self.answered(
            self.store()?
                .put_opts(&Path::from(key), bytes.into().into(), opts),
        )
        .await
        .map_err(StorageError::Put)?;
        Ok(())
    }

    /// Upload an object from a byte stream, without ever holding it whole.
    ///
    /// Bytes are buffered into 5 MiB multipart parts, so peak memory is one
    /// part, and the object becomes visible only once every part landed. A
    /// [`StorageError`] converts into `std::io::Error`, so a stream read out of
    /// storage can be written straight back into it.
    ///
    /// Any failure, or dropping this future mid-upload, aborts the upload.
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
            self.answered(self.store()?.put_multipart_opts(&Path::from(key), opts))
                .await
                .map_err(StorageError::Put)?,
            key,
            self.config.operation_timeout,
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
            // S3 bounds a part only from below, so the chunks ship uncopied.
            let part = upload
                .get()
                .put_part(PutPayload::from_iter(pending.drain(..)));
            if let Err(e) = self.answered(part).await {
                upload.abort().await;
                return Err(StorageError::Put(e));
            }
            pending_len = 0;
        }

        // The tail ships even when empty: a multipart upload with no part at all
        // is rejected on completion, so a zero-byte stream still needs one.
        let tail = upload.get().put_part(PutPayload::from_iter(pending));
        if let Err(e) = self.answered(tail).await {
            upload.abort().await;
            return Err(StorageError::Put(e));
        }
        if let Err(e) = self.answered(upload.get().complete()).await {
            upload.abort().await;
            return Err(StorageError::Put(e));
        }
        upload.finished();
        Ok(())
    }

    /// Delete an object. Absent keys succeed, so retention sweeps and
    /// failed-upload cleanup are idempotent.
    pub async fn delete(&self, key: &str) -> Result<()> {
        match self.answered(self.store()?.delete(&Path::from(key))).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(StorageError::Delete(e)),
        }
    }

    /// `call`'s answer, or S3's error once the operation budget elapses.
    async fn answered<T>(
        &self,
        call: impl Future<Output = object_store::Result<T>>,
    ) -> object_store::Result<T> {
        bounded(self.config.operation_timeout, call)
            .await
            .and_then(|answer| answer)
    }
}

/// `call`'s output, or once `budget` elapses a store error naming the budget
/// and what sets it.
pub(crate) async fn bounded<F: Future>(
    budget: Duration,
    call: F,
) -> object_store::Result<F::Output> {
    #[expect(
        clippy::map_err_ignore,
        reason = "Elapsed carries nothing the timeout's own message does not say"
    )]
    tokio::time::timeout(budget, call)
        .await
        .map_err(|_| object_store::Error::Generic {
            store: "S3",
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "the call did not end within {budget:?}, every retry included — the budget \
                     {} sets",
                    nest_rs_config::var_name("storage", OPERATION_TIMEOUT.key()),
                ),
            )),
        })
}

/// Holds a multipart upload so that a cancelled one is aborted too: S3 bills
/// orphaned parts until a lifecycle rule sweeps them.
///
/// `Drop` cannot await, so it hands the abort to a detached, best-effort task
/// and says so at `warn`, naming the key.
struct UploadGuard {
    upload: Option<Box<dyn MultipartUpload>>,
    key: String,
    /// The unit of work that opened the upload, so the detached abort carries
    /// its `trace_id`. Captured at `new`: a dropped future may not drop on the
    /// task that owned it.
    context: TaskContext,
    /// How long the abort waits for S3, as every other call of the upload does.
    budget: Duration,
}

impl UploadGuard {
    fn new(upload: Box<dyn MultipartUpload>, key: &str, budget: Duration) -> Self {
        Self {
            upload: Some(upload),
            key: key.to_owned(),
            context: TaskContext::current(),
            budget,
        }
    }

    /// The upload itself, for the duration of one call.
    #[expect(
        clippy::expect_used,
        reason = "the upload is taken only by abort or finished, which both consume the guard"
    )]
    fn get(&mut self) -> &mut Box<dyn MultipartUpload> {
        self.upload
            .as_mut()
            .expect("the upload is taken only by `abort` or `finished`, which both consume it")
    }

    /// Abort now and disarm, so an explicit failure path emits its event
    /// before returning.
    async fn abort(&mut self) {
        if let Some(mut upload) = self.upload.take() {
            abort_upload(&mut upload, &self.key, self.budget).await;
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
        let context = self.context.clone();
        let budget = self.budget;
        // A `Drop` cannot prove it runs in a runtime, and panicking while
        // unwinding would replace a billing leak with a crash.
        let runtime = tokio::runtime::Handle::try_current();
        context.span().in_scope(|| {
            tracing::warn!(
                target: crate::TARGET,
                key = key.as_str(),
                "multipart upload was cancelled mid-flight; discarding its parts",
            );
            if runtime.is_err() {
                tracing::warn!(
                    target: crate::TARGET,
                    key = key.as_str(),
                    "no runtime is available to discard them; they are left for the store's \
                     lifecycle rule",
                );
            }
        });
        if let Ok(handle) = runtime {
            handle.spawn(context.carry(async move {
                abort_upload(&mut upload, &key, budget).await;
            }));
        }
    }
}

/// Discard the parts of a multipart upload that will never complete.
///
/// Both outcomes are logged: `object_store` cannot list multipart uploads, so
/// the event is the only trace of whether the parts were discarded.
async fn abort_upload(upload: &mut Box<dyn MultipartUpload>, key: &str, budget: Duration) {
    match bounded(budget, upload.abort())
        .await
        .and_then(|aborted| aborted)
    {
        Ok(()) => tracing::debug!(
            target: crate::TARGET,
            key,
            "discarded the parts of an interrupted multipart upload",
        ),
        Err(error) => tracing::warn!(
            target: crate::TARGET,
            key,
            error = %nest_rs_core::error_message(&error),
            "multipart upload left dangling parts",
        ),
    }
}

/// Result of a [`Storage::head`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectMetadata {
    /// The object's size in bytes, as reported by S3.
    pub byte_size: i64,
}

/// One object yielded by [`Storage::list`].
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
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[test]
    fn the_last_backoff_ends_within_the_budget_whatever_the_budget() {
        for budget in [
            Duration::from_millis(1),
            Duration::from_secs(1),
            StorageConfig::default().operation_timeout,
            Duration::from_secs(60 * 60),
        ] {
            let retries = retries_within(budget);
            assert!(retries.backoff.init_backoff <= retries.backoff.max_backoff);
            assert!(
                retries.retry_timeout + retries.backoff.max_backoff < budget,
                "{budget:?}"
            );
        }
    }

    /// A multipart upload that talks to nothing but records whether it was
    /// aborted.
    #[derive(Debug)]
    struct NeverUploaded(Arc<AtomicBool>);

    #[async_trait::async_trait]
    impl MultipartUpload for NeverUploaded {
        fn put_part(&mut self, _data: object_store::PutPayload) -> object_store::UploadPart {
            Box::pin(async { Ok(()) })
        }

        async fn complete(&mut self) -> object_store::Result<object_store::PutResult> {
            unreachable!("this double is only ever dropped")
        }

        async fn abort(&mut self) -> object_store::Result<()> {
            self.0.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn parts_dropped_outside_a_runtime_are_reported_rather_than_silently_leaked() {
        // A plain thread: a value carried out of a runtime and dropped there.
        let handle = std::thread::spawn(|| {
            let logs = nest_rs_testing::LogCapture::install();
            let aborted = Arc::new(AtomicBool::new(false));
            drop(UploadGuard::new(
                Box::new(NeverUploaded(Arc::clone(&aborted))),
                "uploads/abandoned",
                Duration::from_secs(1),
            ));
            assert!(
                !aborted.load(Ordering::SeqCst),
                "with no runtime there is nowhere to issue the abort — that is \
                 the whole reason the line exists",
            );

            let cancelled = logs.expect_one(
                crate::TARGET,
                "multipart upload was cancelled mid-flight; discarding its parts",
            );
            assert_eq!(cancelled.level, "warn");

            let leaked = logs.expect_one(
                crate::TARGET,
                "no runtime is available to discard them; they are left for the store's \
                 lifecycle rule",
            );
            assert_eq!(leaked.level, "warn");
            assert_eq!(
                leaked.field("key").as_deref(),
                Some("uploads/abandoned"),
                "the key is the whole content of the line — without it nothing \
                 can be reconciled against the bucket: {:?}",
                leaked.fields,
            );
        });
        handle.join().expect("the dropping thread finishes");
    }

    #[tokio::test]
    async fn parts_dropped_inside_a_runtime_are_discarded_rather_than_reported() {
        let logs = nest_rs_testing::LogCapture::install();
        let aborted = Arc::new(AtomicBool::new(false));
        drop(UploadGuard::new(
            Box::new(NeverUploaded(Arc::clone(&aborted))),
            "uploads/cancelled",
            Duration::from_secs(1),
        ));
        // The abort is spawned: yield, bounded, until it lands.
        for _ in 0..100 {
            if aborted.load(Ordering::SeqCst) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            aborted.load(Ordering::SeqCst),
            "the parts really were discarded — asserting only that the fallback \
             line is absent passes just as well for a branch that does nothing",
        );

        assert!(
            logs.find(
                crate::TARGET,
                "no runtime is available to discard them; they are left for the store's \
                 lifecycle rule",
            )
            .is_empty(),
            "…and it said nothing about leaving them behind: {:#?}",
            logs.events(),
        );
    }

    fn client(endpoint: &str, allow_http: bool) -> Storage {
        Storage::new(Arc::new(StorageConfig {
            endpoint: endpoint.into(),
            allow_http,
            ..Default::default()
        }))
    }

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
            assert!(
                err.to_string()
                    .contains(&nest_rs_config::var_name("storage", "ALLOW_HTTP"))
            );
        }
    }

    #[tokio::test]
    async fn presigning_over_an_encrypted_endpoint_is_untouched() {
        // Signing is local: no server is needed.
        let storage = client("https://s3.example", false);
        storage
            .presign_get("k", Duration::from_secs(900))
            .await
            .expect("https is always allowed");
    }

    /// A config built in code skips `from_env`: the client refuses what the
    /// read would have, before anything is signed or sent.
    #[tokio::test]
    async fn a_hand_built_client_refuses_tls_material_it_cannot_use() {
        let issued = nest_rs_testing::TestAuthority::new().client("nestrs-test-client");
        let inline = |pem: &str| nest_rs_config::Material {
            bytes: pem.as_bytes().to_vec(),
            path: None,
        };
        for (tls, names) in [
            (
                nest_rs_config::ClientTls::new(
                    None,
                    Some(nest_rs_config::TlsIdentity::new(
                        inline(&issued.cert),
                        inline(&issued.key),
                    )),
                ),
                "TLS_CERT",
            ),
            (
                nest_rs_config::ClientTls::new(Some(inline("no certificate")), None),
                "TLS_CA_CERT",
            ),
        ] {
            let storage = Storage::new(Arc::new(StorageConfig {
                endpoint: "https://s3.example".into(),
                tls,
                ..Default::default()
            }));
            let err = storage
                .presign_get("k", Duration::from_secs(900))
                .await
                .expect_err("material the client cannot use mints nothing");
            let shown = nest_rs_core::error_message(&err);
            assert!(
                matches!(err, StorageError::Init(_))
                    && shown.contains(&nest_rs_config::var_name("storage", names)),
                "{shown}"
            );
        }
    }

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

    /// An S3 that accepts every connection and never answers. The clock stops
    /// at the first accept, so the budget is waited out for free.
    async fn silent_store() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local listener");
        let addr = listener.local_addr().expect("a bound address");
        tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                let (socket, _) = listener.accept().await.expect("accept");
                if held.is_empty() {
                    tokio::time::pause();
                }
                held.push(socket);
            }
        });
        format!("http://{addr}")
    }

    /// A store that answers its first request with a `503`, then stops the
    /// clock and answers nothing more, so the budget cuts the retry.
    async fn store_answering_once() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local listener");
        let addr = listener.local_addr().expect("a bound address");
        tokio::spawn(async move {
            let mut held = Vec::new();
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let read = socket.read(&mut buf).await.expect("read the request");
                assert!(read > 0, "the client hung up before its request ended");
                request.extend_from_slice(&buf[..read]);
            }
            socket
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n")
                .await
                .expect("answer the first request");
            tokio::time::pause();
            held.push(socket);
            loop {
                let (socket, _) = listener.accept().await.expect("accept");
                held.push(socket);
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn a_store_that_answered_before_the_cut_is_not_said_to_have_been_silent() {
        let budget = StorageConfig::default().operation_timeout;
        let storage = client(&store_answering_once().await, true);
        let refused = tokio::time::timeout(budget * 2, storage.head("k"))
            .await
            .expect("cut within the budget")
            .expect_err("a store answering only a 503 returns nothing");
        let chain = nest_rs_core::error_message(&refused);
        assert!(
            chain.contains(&format!(
                "the call did not end within {budget:?}, every retry included — the budget {} sets",
                nest_rs_config::var_name("storage", "OPERATION_TIMEOUT_SECS")
            )) && !chain.contains("did not answer"),
            "{chain}"
        );
    }

    #[tokio::test]
    async fn a_call_whose_attempts_keep_failing_ends_on_their_cause_before_the_budget() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a port nothing listens on once its listener is dropped");
        let budget = Duration::from_secs(1);
        let storage = Storage::new(Arc::new(StorageConfig {
            endpoint: format!("http://{closed}"),
            allow_http: true,
            operation_timeout: budget,
            ..StorageConfig::default()
        }));
        // The client is built on first use, reading the system's authorities
        // once per process — not the call's to wait out. Signing is local.
        storage
            .presign_get("k", budget)
            .await
            .expect("the client builds");
        let started = tokio::time::Instant::now();
        let refused = storage.head("k").await.expect_err("nothing listens");
        let chain = nest_rs_core::error_message(&refused);
        assert!(started.elapsed() < budget, "{:?}", started.elapsed());
        assert!(
            !chain.contains("did not end within") && chain.contains("retries"),
            "{chain}"
        );
    }

    type Call = fn(Storage) -> futures_util::future::BoxFuture<'static, Result<()>>;

    /// Every call that waits on S3, each on a store of its own, since a second
    /// connection made on a stopped clock races the clock's jump.
    const CALLS: [(&str, Call); 7] = [
        ("head", |s| {
            Box::pin(async move { s.head("k").await.map(drop) })
        }),
        ("get_bytes", |s| {
            Box::pin(async move { s.get_bytes("k").await.map(drop) })
        }),
        ("get_stream", |s| {
            Box::pin(async move { s.get_stream("k").await.map(drop) })
        }),
        ("list", |s| {
            Box::pin(async move {
                use futures_util::StreamExt;
                s.list("")?.next().await.transpose().map(drop)
            })
        }),
        ("put_bytes", |s| {
            Box::pin(async move { s.put_bytes("k", Bytes::new(), "text/plain").await })
        }),
        ("put_stream", |s| {
            Box::pin(async move {
                s.put_stream("k", "text/plain", futures_util::stream::empty())
                    .await
            })
        }),
        ("delete", |s| Box::pin(async move { s.delete("k").await })),
    ];

    #[tokio::test]
    async fn every_call_s3_never_answers_fails_at_the_operation_budget_naming_it() {
        let budget = StorageConfig::default().operation_timeout;
        for (name, call) in CALLS {
            let storage = client(&silent_store().await, true);
            let sent = tokio::time::Instant::now();
            let refused = tokio::time::timeout(budget * 2, call(storage))
                .await
                .unwrap_or_else(|_| panic!("{name}: no answer within twice the budget"))
                .expect_err("a store that never answers returns nothing");
            let chain = nest_rs_core::error_message(&refused);
            // The handshake before the clock stopped took real time.
            let waited = sent.elapsed();
            assert!(
                waited >= budget && waited < budget + Duration::from_secs(1),
                "{name}: {waited:?}"
            );
            assert!(
                chain.contains(&format!(
                    "the call did not end within {budget:?}, every retry included — the budget {} \
                     sets",
                    nest_rs_config::var_name("storage", "OPERATION_TIMEOUT_SECS")
                )),
                "{name}: {chain}"
            );
            tokio::time::resume();
        }
    }

    #[tokio::test]
    async fn a_hand_built_client_refuses_a_zero_budget_naming_its_field() {
        let storage = Storage::new(Arc::new(StorageConfig {
            operation_timeout: Duration::ZERO,
            ..Default::default()
        }));
        let refused = storage
            .head("k")
            .await
            .expect_err("refused before any call");
        assert!(matches!(refused, StorageError::Init(_)), "{refused:?}");
        let chain = nest_rs_core::error_message(&refused);
        assert!(
            chain.contains("StorageConfig::operation_timeout"),
            "{chain}"
        );
    }

    // `Body::from_bytes_stream` fed from `get_stream` needs `E: Into<std::io::Error>`.
    #[test]
    fn a_storage_error_converts_into_io_error_so_streams_compose() {
        let io: std::io::Error = StorageError::PlaintextEndpoint {
            endpoint: "http://x".into(),
        }
        .into();
        assert!(io.to_string().contains("plain HTTP"));
    }
}

use std::any::TypeId;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::Method;
use nest_rs_config::ConfigModule;
use nest_rs_core::{Budget, ContainerBuilder, Discoverable, ProviderResidency, TaskContext};
use object_store::path::Path;
use object_store::signer::Signer;
use object_store::{
    Attribute, Attributes, MultipartUpload, ObjectStoreExt, PutMultipartOptions, PutOptions,
    PutPayload,
};

use crate::config::{OPERATION_TIMEOUT, StorageConfig};
use crate::error::{Result, StorageError};
use crate::store::Store;
use crate::transfer::download;

/// Bytes buffered before a multipart part is shipped: S3 requires every part
/// but the last to be at least 5 MiB.
pub const MULTIPART_PART_SIZE: usize = 5 * 1024 * 1024;

/// The injectable object-store client: the store [`StorageConfig::url`] names,
/// opened over the [`object_store`] backend its scheme picks and proved when
/// it opens.
///
/// Every call waits on the store within [`StorageConfig::operation_timeout`],
/// and a download, while it is read, for its next bytes within
/// [`StorageConfig::read_timeout`].
pub struct Storage {
    config: Arc<StorageConfig>,
    store: Store,
}

// Registered by hand rather than by `#[injectable]` for what the decorator
// cannot say: the store opened by `StorageModule`'s factory, and the client's
// budget, held under every net reaching it.
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
        let container = builder.snapshot();
        let config = match ConfigModule::resolved::<StorageConfig>(&container) {
            Ok(config) => config,
            Err(unresolved) => return builder.refuse(unresolved),
        };
        let Some(store) = container.get::<Store>() else {
            return builder.refuse(anyhow::anyhow!(
                "Storage: the object store was not opened — import `StorageModule`, whose \
                 factory opens it"
            ));
        };
        let storage = Self {
            config,
            store: Store::clone(&store),
        };
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

    fn __nestrs_register_overridden(builder: ContainerBuilder) -> ContainerBuilder {
        nest_rs_core::__private::claim_own_override::<Self>(
            builder,
            nest_rs_core::ProviderScope::Singleton,
        )
    }
}

impl ProviderResidency for Storage {
    const SINGLETON: bool = true;
}

impl Storage {
    /// Open the store `config` names and prove it: an `s3://` bucket answers a
    /// listing of its prefix within the operation budget, or the client is
    /// refused naming what to change — what the boot does for an app
    /// importing [`StorageModule`](crate::StorageModule).
    pub async fn connect(config: &StorageConfig) -> Result<Self> {
        Ok(Self {
            store: Store::open(config).await?,
            config: Arc::new(config.clone()),
        })
    }

    /// The store's URL, shown without what a URL may carry beside its bucket
    /// and prefix.
    pub fn url(&self) -> &str {
        &self.store.url
    }

    /// Sign a short-lived URL for `method` against `key`.
    async fn presigned_url(&self, method: Method, key: &str, expires: Duration) -> Result<String> {
        let signer = self.store.signer()?;
        let url = self
            .answered(signer.signed_url(method.clone(), &self.store.rooted(key), expires))
            .await
            .map_err(|source| StorageError::Presign {
                method,
                store: self.store.shown.to_string(),
                source: Box::new(source),
            })?;
        Ok(url.to_string())
    }

    /// Presigned `PUT` URL the client uploads bytes to directly; the content
    /// type is not signed, so the uploading client sets it. A backend that
    /// cannot sign — `memory` — answers [`StorageError::Unsupported`].
    pub async fn presign_put(&self, key: &str, expires: Duration) -> Result<String> {
        self.presigned_url(Method::PUT, key, expires).await
    }

    /// Presigned `GET` URL — for serving private originals on demand. A
    /// backend that cannot sign — `memory` — answers
    /// [`StorageError::Unsupported`].
    pub async fn presign_get(&self, key: &str, expires: Duration) -> Result<String> {
        self.presigned_url(Method::GET, key, expires).await
    }

    /// Byte size of an uploaded object, `None` if it does not exist yet.
    ///
    /// `object_store` does not report the stored `Content-Type`: a caller that
    /// needs it keeps the one it supplied at upload.
    pub async fn head(&self, key: &str) -> Result<Option<ObjectMetadata>> {
        match self
            .answered(self.store.objects.head(&Path::from(key)))
            .await
        {
            Ok(meta) => Ok(Some(ObjectMetadata {
                byte_size: meta.size as i64,
            })),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(StorageError::Head {
                store: self.store.shown.to_string(),
                source: Box::new(e),
            }),
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

    /// The object at `key`'s size, and its bytes as the store sends them.
    async fn transfer(
        &self,
        key: &str,
    ) -> Result<(
        u64,
        impl futures_util::Stream<Item = Result<Bytes>> + Send + 'static + use<>,
    )> {
        let path = Path::from(key);
        let first = self
            .answered(self.store.objects.get(&path))
            .await
            .map_err(|source| StorageError::Get {
                store: self.store.shown.to_string(),
                source: Box::new(source),
            })?;
        let size = first.range.end - first.range.start;
        Ok((
            size,
            download(
                self.store.clone(),
                path,
                first,
                self.config.read_timeout,
                self.config.operation_timeout,
            ),
        ))
    }

    /// List the objects stored under `prefix`, one entry at a time, fetching
    /// the next page as the stream is consumed. Pass `""` for every key the
    /// store's URL reaches; a key is read relative to the URL's own prefix.
    ///
    /// `prefix` matches on **path segments**, not on characters: `posts/cover`
    /// is a prefix of `posts/cover/a.png` but not of `posts/cover-2.png`. The
    /// match is recursive, so nested keys are included.
    pub fn list(
        &self,
        prefix: &str,
    ) -> impl futures_util::Stream<Item = Result<ObjectEntry>> + Send + 'static + use<> {
        use futures_util::StreamExt;
        let (budget, backend, shown) = (
            self.config.operation_timeout,
            self.store.backend,
            self.store.shown.clone(),
        );
        let prefix = Path::from(prefix);
        let pages = self
            .store
            .objects
            .list((!prefix.as_ref().is_empty()).then_some(&prefix));
        let entries = Box::pin(futures_util::stream::unfold(
            Some(pages),
            move |pages| async move {
                let mut pages = pages?;
                let entry = match bounded(budget, backend, pages.next()).await {
                    Ok(None) => return None,
                    Ok(Some(entry)) => entry,
                    Err(unanswered) => return Some((Err(unanswered), None)),
                };
                Some((entry, Some(pages)))
            },
        ));
        entries.map(move |meta| match meta {
            Ok(meta) => Ok(ObjectEntry {
                key: meta.location.as_ref().to_string(),
                byte_size: meta.size as i64,
                last_modified: meta.last_modified.into(),
            }),
            Err(e) => Err(StorageError::List {
                store: shown.to_string(),
                source: Box::new(e),
            }),
        })
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
            self.store
                .objects
                .put_opts(&Path::from(key), bytes.into().into(), opts),
        )
        .await
        .map_err(|e| self.put_failed(e))?;
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
            self.answered(
                self.store
                    .objects
                    .put_multipart_opts(&Path::from(key), opts),
            )
            .await
            .map_err(|e| self.put_failed(e))?,
            key,
            self.config.operation_timeout,
            self.store.backend,
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
                return Err(self.put_failed(e));
            }
            pending_len = 0;
        }

        // The tail ships even when empty: a multipart upload with no part at all
        // is rejected on completion, so a zero-byte stream still needs one.
        let tail = upload.get().put_part(PutPayload::from_iter(pending));
        if let Err(e) = self.answered(tail).await {
            upload.abort().await;
            return Err(self.put_failed(e));
        }
        if let Err(e) = self.answered(upload.get().complete()).await {
            upload.abort().await;
            return Err(self.put_failed(e));
        }
        upload.finished();
        Ok(())
    }

    /// Delete an object. Absent keys succeed, so retention sweeps and
    /// failed-upload cleanup are idempotent.
    pub async fn delete(&self, key: &str) -> Result<()> {
        match self
            .answered(self.store.objects.delete(&Path::from(key)))
            .await
        {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(StorageError::Delete {
                store: self.store.shown.to_string(),
                source: Box::new(e),
            }),
        }
    }

    /// An upload's failure, naming the store.
    fn put_failed(&self, error: object_store::Error) -> StorageError {
        StorageError::Put {
            store: self.store.shown.to_string(),
            source: Box::new(error),
        }
    }

    /// `call`'s answer, or the store's error once the operation budget elapses.
    async fn answered<T>(
        &self,
        call: impl Future<Output = object_store::Result<T>>,
    ) -> object_store::Result<T> {
        bounded(self.config.operation_timeout, self.store.backend, call)
            .await
            .and_then(|answer| answer)
    }
}

/// `call`'s output, or once `budget` elapses an error of the `backend` store
/// naming the budget and what sets it.
pub(crate) async fn bounded<F: Future>(
    budget: Duration,
    backend: &'static str,
    call: F,
) -> object_store::Result<F::Output> {
    #[expect(
        clippy::map_err_ignore,
        reason = "Elapsed carries nothing the timeout's own message does not say"
    )]
    tokio::time::timeout(budget, call)
        .await
        .map_err(|_| object_store::Error::Generic {
            store: backend,
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
    /// How long the abort waits for the store, as every other call of the
    /// upload does.
    budget: Duration,
    /// The backend the upload goes to.
    backend: &'static str,
}

impl UploadGuard {
    fn new(
        upload: Box<dyn MultipartUpload>,
        key: &str,
        budget: Duration,
        backend: &'static str,
    ) -> Self {
        Self {
            upload: Some(upload),
            key: key.to_owned(),
            context: TaskContext::current(),
            budget,
            backend,
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
            abort_upload(&mut upload, &self.key, self.budget, self.backend).await;
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
        let (budget, backend) = (self.budget, self.backend);
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
                abort_upload(&mut upload, &key, budget, backend).await;
            }));
        }
    }
}

/// Discard the parts of a multipart upload that will never complete.
///
/// Both outcomes are logged: `object_store` cannot list multipart uploads, so
/// the event is the only trace of whether the parts were discarded.
async fn abort_upload(
    upload: &mut Box<dyn MultipartUpload>,
    key: &str,
    budget: Duration,
    backend: &'static str,
) {
    match bounded(budget, backend, upload.abort())
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
#[non_exhaustive]
pub struct ObjectMetadata {
    /// The object's size in bytes, as the store reports it.
    pub byte_size: i64,
}

/// One object yielded by [`Storage::list`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ObjectEntry {
    /// The object's key, relative to the prefix of the store's URL.
    pub key: String,
    /// The object's size in bytes, as the store reports it.
    pub byte_size: i64,
    /// When the object was last written.
    pub last_modified: SystemTime,
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    impl Storage {
        /// A client over the store `config` names, opened without its proof,
        /// for the tests of what a call does once it is open.
        fn unproved(config: StorageConfig) -> Self {
            Self {
                store: Store::built(&config).expect("the store builds").0,
                config: Arc::new(config),
            }
        }
    }

    /// An `s3://` config reaching `endpoint` with a pair of its own.
    fn s3_at(endpoint: &str, allow_http: bool) -> StorageConfig {
        StorageConfig {
            url: "s3://bucket".into(),
            endpoint: Some(endpoint.into()),
            access_key: "AKIATEST".into(),
            secret_key: "test-secret".into(),
            allow_http,
            ..StorageConfig::default()
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
                "s3",
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
            "s3",
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

    #[tokio::test]
    async fn a_plaintext_endpoint_is_refused_when_http_is_disallowed_naming_the_opt_in() {
        let Err(refused) = Storage::connect(&s3_at("http://k:s3cr3t@minio:9000", false)).await
        else {
            panic!("a plaintext presigned URL must never be minted");
        };
        assert!(
            matches!(refused, StorageError::PlaintextEndpoint { .. }),
            "got {refused:?}",
        );
        let shown = refused.to_string();
        assert!(
            shown.contains(&nest_rs_config::var_name("storage", "ALLOW_HTTP"))
                && shown.contains("http://minio:9000/")
                && !shown.contains("s3cr3t"),
            "{shown}"
        );
    }

    #[tokio::test]
    async fn presigning_over_an_encrypted_endpoint_is_untouched() {
        // Signing is local: no server is needed.
        let storage = Storage::unproved(s3_at("https://s3.example", false));
        let signed = storage
            .presign_get("k", Duration::from_secs(900))
            .await
            .expect("https is always allowed");
        assert!(
            signed.starts_with("https://s3.example/bucket/k?"),
            "{signed}"
        );
    }

    #[tokio::test]
    async fn a_presigned_key_is_addressed_under_the_urls_prefix() {
        let storage = Storage::unproved(StorageConfig {
            url: "s3://bucket/tenants/acme".into(),
            ..s3_at("https://s3.example", false)
        });
        let signed = storage
            .presign_put("covers/a.png", Duration::from_secs(900))
            .await
            .expect("signing is local");
        assert!(
            signed.starts_with("https://s3.example/bucket/tenants/acme/covers/a.png?"),
            "{signed}"
        );
    }

    /// A config built in code skips `from_env`: the client refuses what the
    /// read would have, before anything is signed or sent.
    #[tokio::test]
    async fn a_client_built_in_code_refuses_what_the_boot_refuses_naming_the_variables() {
        let issued = nest_rs_testing::TestAuthority::new().client("nestrs-test-client");
        let inline = |pem: &str| nest_rs_config::Material {
            bytes: pem.as_bytes().to_vec(),
            path: None,
        };
        for (config, names) in [
            (
                StorageConfig {
                    tls: nest_rs_config::ClientTls::new(
                        None,
                        Some(nest_rs_config::TlsIdentity::new(
                            inline(&issued.cert),
                            inline(&issued.key),
                        )),
                    ),
                    ..s3_at("https://s3.example", false)
                },
                "TLS_CERT",
            ),
            (
                StorageConfig {
                    tls: nest_rs_config::ClientTls::new(Some(inline("no certificate")), None),
                    ..s3_at("https://s3.example", false)
                },
                "TLS_CA_CERT",
            ),
            (
                StorageConfig {
                    operation_timeout: Duration::ZERO,
                    ..s3_at("https://s3.example", false)
                },
                "OPERATION_TIMEOUT_SECS",
            ),
            (
                StorageConfig {
                    secret_key: String::new(),
                    ..s3_at("https://s3.example", false)
                },
                "SECRET_KEY",
            ),
            (
                StorageConfig {
                    url: "gs://bucket".into(),
                    ..s3_at("https://s3.example", false)
                },
                "URL",
            ),
        ] {
            let Err(refused) = Storage::connect(&config).await else {
                panic!("{names}: a config the boot refuses opens nothing");
            };
            let shown = nest_rs_core::error_message(&refused);
            assert!(
                matches!(refused, StorageError::Init(_))
                    && (shown.contains(&nest_rs_config::var_name("storage", names))
                        || shown.contains("StorageConfig::operation_timeout")),
                "{names}: {shown}"
            );
        }
    }

    #[tokio::test]
    async fn memory_signs_nothing_and_says_which_backend_cannot() {
        nest_rs_testing::load_project_env();
        let storage = Storage::connect(&StorageConfig {
            url: "memory:///".into(),
            ..StorageConfig::default()
        })
        .await
        .expect("memory opens in a test profile");
        let refused = storage
            .presign_get("k", Duration::from_secs(60))
            .await
            .expect_err("no signer");
        assert!(
            matches!(
                refused,
                StorageError::Unsupported {
                    operation: "presign",
                    backend: "memory"
                }
            ),
            "{refused:?}"
        );
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
        let storage = Storage::unproved(s3_at(&store_answering_once().await, true));
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
        let storage = Storage::unproved(StorageConfig {
            operation_timeout: budget,
            ..s3_at(&format!("http://{closed}"), true)
        });
        let started = tokio::time::Instant::now();
        let refused = storage.head("k").await.expect_err("nothing listens");
        let chain = nest_rs_core::error_message(&refused);
        assert!(started.elapsed() < budget, "{:?}", started.elapsed());
        assert!(
            !chain.contains("did not end within") && chain.contains("could not connect"),
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
                s.list("").next().await.transpose().map(drop)
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

    /// The client is opened before the clock is read, so what the call waits
    /// is the budget alone: the dial before the clock stops is the only real
    /// time it holds.
    #[tokio::test]
    async fn every_call_s3_never_answers_fails_at_the_operation_budget_naming_it() {
        let budget = StorageConfig::default().operation_timeout;
        for (name, call) in CALLS {
            let storage = Storage::unproved(s3_at(&silent_store().await, true));
            let sent = tokio::time::Instant::now();
            let refused = tokio::time::timeout(budget * 2, call(storage))
                .await
                .unwrap_or_else(|_| panic!("{name}: no answer within twice the budget"))
                .expect_err("a store that never answers returns nothing");
            let chain = nest_rs_core::error_message(&refused);
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

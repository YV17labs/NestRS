//! A download's body: bounded by its stall while its reader waits, never by its
//! size, and resumed from where it stopped, however long it runs.
//!
//! The bound is this crate's, not the HTTP client's: reqwest's read timeout
//! keeps running between two reads of a body, so it cuts a reader that pauses
//! — a slow client behind a streamed response — and its per-request timer runs
//! through an upload's body too.

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt};
use object_store::path::Path;
use object_store::{GetOptions, GetRange, GetResult, ObjectStore};

use crate::client::bounded;
use crate::config::READ_TIMEOUT;
use crate::error::{Result, StorageError};

/// The object at `path` from the answer `first` S3 gave, read within
/// `read_timeout` per wait for its next bytes.
///
/// A body silent that long while its reader waits, or broken past
/// `object_store`'s own resumption, is resumed with a ranged `GET` of what is
/// still owed, said at `warn`, and fenced on the object's `ETag` — sent as
/// `If-Match` and checked on the answer — so a version written since is
/// refused rather than spliced in; each resume waits for S3's answer within
/// `operation_timeout`. A resumed body that stops before its
/// first byte fails the download on what stopped it — a stall naming the
/// bound.
pub(crate) fn download(
    store: Arc<dyn ObjectStore>,
    path: Path,
    first: GetResult,
    read_timeout: Duration,
    operation_timeout: Duration,
) -> impl Stream<Item = Result<Bytes>> + Send + 'static + use<> {
    let transfer = Transfer {
        e_tag: first.meta.e_tag.clone(),
        owed: first.range.clone(),
        body: first.into_stream(),
        store,
        path,
        resumable: true,
        read_timeout,
        operation_timeout,
    };
    futures_util::stream::unfold(
        Some(transfer),
        |transfer| async move { transfer?.next().await },
    )
    .fuse()
}

/// One download in flight.
struct Transfer {
    store: Arc<dyn ObjectStore>,
    path: Path,
    /// The version every resumed request is fenced on.
    e_tag: Option<String>,
    /// The bytes not yet read, as offsets into the object.
    owed: Range<u64>,
    body: BoxStream<'static, object_store::Result<Bytes>>,
    /// Whether a body stopping now is resumed: the first body always, a
    /// resumed one once it sent a byte.
    resumable: bool,
    read_timeout: Duration,
    operation_timeout: Duration,
}

impl Transfer {
    /// The next chunk, and the transfer still owing more — `None` once the
    /// object is read or the download failed.
    async fn next(mut self) -> Option<(Result<Bytes>, Option<Self>)> {
        loop {
            let stopped = match tokio::time::timeout(self.read_timeout, self.body.next()).await {
                Ok(Some(Ok(chunk))) => {
                    self.owed.start += chunk.len() as u64;
                    self.resumable = true;
                    return Some((Ok(chunk), Some(self)));
                }
                Ok(None) => return None,
                Err(_) if self.owed.is_empty() => return None,
                Ok(Some(Err(error))) => error,
                Err(_) => stalled(self.read_timeout),
            };
            if let Err(error) = self.resume(stopped).await {
                return Some((Err(StorageError::Get(error)), None));
            }
        }
    }

    /// Replace a body that stopped on `stopped` with S3's answer for what is
    /// still owed — or end the download on `stopped` when a resumed body
    /// stopped before its first byte, or no `ETag` fences a resume.
    async fn resume(&mut self, stopped: object_store::Error) -> object_store::Result<()> {
        let (true, Some(e_tag)) = (self.resumable, self.e_tag.clone()) else {
            return Err(stopped);
        };
        tracing::warn!(
            target: crate::TARGET,
            key = self.path.as_ref(),
            error = %nest_rs_core::error_message(&stopped),
            "download resumed where its body stopped",
        );
        let options = GetOptions {
            if_match: Some(e_tag.clone()),
            range: Some(GetRange::Bounded(self.owed.clone())),
            ..GetOptions::default()
        };
        let answer = bounded(
            self.operation_timeout,
            self.store.get_opts(&self.path, options),
        )
        .await??;
        // Checked here too, as object_store checks its own resumes: a store
        // that ignores `If-Match` would hand over another version's rest.
        if answer.meta.e_tag.as_deref() != Some(e_tag.as_str()) {
            return Err(object_store::Error::Precondition {
                path: self.path.to_string(),
                source: "the object changed while it was read, so the rest of another version \
                         is not spliced onto what was read"
                    .into(),
            });
        }
        self.body = answer.into_stream();
        self.resumable = false;
        Ok(())
    }
}

/// The error a download ends on when its body stays silent past the read
/// bound, naming the bound and what sets it.
fn stalled(read_timeout: Duration) -> object_store::Error {
    object_store::Error::Generic {
        store: "S3",
        source: Box::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!(
                "the download sent nothing for {read_timeout:?} while it was read — the bound {} \
                 sets",
                nest_rs_config::var_name("storage", READ_TIMEOUT.key()),
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use futures_util::stream::BoxStream;
    use object_store::memory::InMemory;
    use object_store::{
        CopyOptions, GetResultPayload, ListResult, MultipartUpload, ObjectMeta, ObjectStoreExt,
        PutMultipartOptions, PutOptions, PutPayload, PutResult,
    };

    use super::*;

    /// A store honouring ranges and ignoring `If-Match`, as an S3-compatible
    /// server may.
    #[derive(Debug)]
    struct FenceIgnoring(InMemory);

    impl std::fmt::Display for FenceIgnoring {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("FenceIgnoring")
        }
    }

    #[async_trait::async_trait]
    impl ObjectStore for FenceIgnoring {
        async fn put_opts(
            &self,
            location: &Path,
            payload: PutPayload,
            opts: PutOptions,
        ) -> object_store::Result<PutResult> {
            self.0.put_opts(location, payload, opts).await
        }

        async fn put_multipart_opts(
            &self,
            location: &Path,
            opts: PutMultipartOptions,
        ) -> object_store::Result<Box<dyn MultipartUpload>> {
            self.0.put_multipart_opts(location, opts).await
        }

        async fn get_opts(
            &self,
            location: &Path,
            options: GetOptions,
        ) -> object_store::Result<GetResult> {
            let options = GetOptions {
                if_match: None,
                ..options
            };
            self.0.get_opts(location, options).await
        }

        fn delete_stream(
            &self,
            locations: BoxStream<'static, object_store::Result<Path>>,
        ) -> BoxStream<'static, object_store::Result<Path>> {
            self.0.delete_stream(locations)
        }

        fn list(
            &self,
            prefix: Option<&Path>,
        ) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
            self.0.list(prefix)
        }

        async fn list_with_delimiter(
            &self,
            prefix: Option<&Path>,
        ) -> object_store::Result<ListResult> {
            self.0.list_with_delimiter(prefix).await
        }

        async fn copy_opts(
            &self,
            from: &Path,
            to: &Path,
            options: CopyOptions,
        ) -> object_store::Result<()> {
            self.0.copy_opts(from, to, options).await
        }
    }

    /// A resume is fenced on the version the download started on: the rest of
    /// a version written since is refused, never spliced onto what was read,
    /// even from a store that ignores the fence.
    #[tokio::test]
    async fn a_resume_answered_from_a_version_written_since_is_refused_not_spliced() {
        let store = Arc::new(FenceIgnoring(InMemory::new()));
        let path = Path::from("k");
        store
            .put(&path, PutPayload::from(vec![1_u8; 100]))
            .await
            .expect("the first version");
        let GetResult {
            meta,
            range,
            attributes,
            extensions,
            ..
        } = store.get(&path).await.expect("the first answer");
        let broken = futures_util::stream::iter([
            Ok(Bytes::from(vec![1_u8; 10])),
            Err(object_store::Error::Generic {
                store: "test",
                source: "the connection broke".into(),
            }),
        ])
        .boxed();
        let first = GetResult {
            payload: GetResultPayload::Stream(broken),
            meta,
            range,
            attributes,
            extensions,
        };
        store
            .put(&path, PutPayload::from(vec![2_u8; 100]))
            .await
            .expect("a version written while the first is read");

        let read: Vec<Result<Bytes>> = download(
            store,
            path,
            first,
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .collect()
        .await;
        let refused = read
            .into_iter()
            .find_map(std::result::Result::err)
            .expect("the rest of another version is refused");
        let chain = nest_rs_core::error_message(&refused);
        assert!(chain.contains("changed while it was read"), "{chain}");
    }
}

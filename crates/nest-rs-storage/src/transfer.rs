//! A download's body: bounded by its stall while its reader waits, never by its
//! size, and resumed from where it stopped.
//!
//! The bound is this crate's, not the HTTP client's: reqwest's read timeout
//! keeps running between two reads of a body, so it cuts a reader that pauses
//! — a slow client behind a streamed response — and its per-request timer runs
//! through an upload's body too.

use std::ops::Range;
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt};
use object_store::aws::AmazonS3;
use object_store::path::Path;
use object_store::{GetOptions, GetRange, GetResult, ObjectStore};

use crate::client::bounded;
use crate::config::READ_TIMEOUT;
use crate::error::{Result, StorageError};

/// The object at `path` from the answer `first` S3 gave, read within
/// `read_timeout` per wait for its next bytes.
///
/// A body silent that long while its reader waits is cut and resumed with a
/// ranged `GET` of what is still owed, fenced on the object's `ETag` so a
/// version written since is refused rather than spliced in; each resume waits
/// for S3's answer within `operation_timeout`. A resumed body that stalls
/// before its first byte fails the download, naming the bound.
pub(crate) fn download(
    store: AmazonS3,
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
    store: AmazonS3,
    path: Path,
    /// The version every resumed request is fenced on.
    e_tag: Option<String>,
    /// The bytes not yet read, as offsets into the object.
    owed: Range<u64>,
    body: BoxStream<'static, object_store::Result<Bytes>>,
    /// Whether a stall now is resumed: the first body's always, a resumed
    /// body's once it sent a byte.
    resumable: bool,
    read_timeout: Duration,
    operation_timeout: Duration,
}

impl Transfer {
    /// The next chunk, and the transfer still owing more — `None` once the
    /// object is read or the download failed.
    async fn next(mut self) -> Option<(Result<Bytes>, Option<Self>)> {
        loop {
            match tokio::time::timeout(self.read_timeout, self.body.next()).await {
                Ok(Some(Ok(chunk))) => {
                    self.owed.start += chunk.len() as u64;
                    self.resumable = true;
                    return Some((Ok(chunk), Some(self)));
                }
                Ok(Some(Err(error))) => return Some((Err(StorageError::Get(error)), None)),
                Ok(None) => return None,
                Err(_) if self.owed.is_empty() => return None,
                Err(_) => {
                    if let Err(error) = self.resume().await {
                        return Some((Err(StorageError::Get(error)), None));
                    }
                }
            }
        }
    }

    /// Replace a stalled body with S3's answer for what is still owed.
    async fn resume(&mut self) -> object_store::Result<()> {
        let (true, Some(e_tag)) = (self.resumable, self.e_tag.clone()) else {
            return Err(stalled(self.read_timeout));
        };
        let options = GetOptions {
            if_match: Some(e_tag),
            range: Some(GetRange::Bounded(self.owed.clone())),
            ..GetOptions::default()
        };
        let answer = bounded(
            self.operation_timeout,
            self.store.get_opts(&self.path, options),
        )
        .await??;
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

//! [`Body`]: the bytes of a request or a response, as frames.

use std::any::Any;
use std::convert::Infallible;
use std::fmt;
use std::io;
use std::pin::Pin;
use std::sync::{Mutex, PoisonError};
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use futures_util::{Stream, TryStreamExt};
use http_body::{Frame, SizeHint};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyDataStream, BodyExt, Empty, Full, StreamBody};

use crate::error::{BodyError, BoxError};

/// A request or response body: frames of [`Bytes`], with an exact size when one
/// is known.
///
/// ```
/// use nest_rs_http::Body;
///
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// let body = Body::from("hello");
/// assert_eq!(body.into_string().await?, "hello");
/// # Ok(())
/// # }
/// ```
pub struct Body(BoxBody<Bytes, io::Error>);

impl Body {
    /// A body with no bytes.
    ///
    /// ```
    /// use nest_rs_http::Body;
    ///
    /// assert!(Body::empty().is_empty());
    /// ```
    pub fn empty() -> Self {
        Self(BoxBody::new(Empty::new().map_err(never)))
    }

    /// A body holding `bytes`, its size exact.
    ///
    /// ```
    /// use nest_rs_http::{Body, Bytes};
    ///
    /// assert!(!Body::from_bytes(Bytes::from_static(b"ok")).is_empty());
    /// ```
    pub fn from_bytes(bytes: Bytes) -> Self {
        Self(BoxBody::new(Full::new(bytes).map_err(never)))
    }

    /// A body holding `text`'s UTF-8 bytes.
    ///
    /// ```
    /// use nest_rs_http::Body;
    ///
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// let body = Body::from_string(String::from("ada"));
    /// assert_eq!(body.into_bytes().await?, "ada");
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_string(text: String) -> Self {
        Self::from_bytes(Bytes::from(text))
    }

    /// A body read from `stream`, one data frame per item; an item's error ends
    /// the body as an [`io::Error`].
    ///
    /// ```
    /// use nest_rs_http::{Body, Bytes};
    ///
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// let chunks = futures_util::stream::iter([
    ///     Ok::<_, std::io::Error>(Bytes::from_static(b"he")),
    ///     Ok(Bytes::from_static(b"llo")),
    /// ]);
    /// assert_eq!(Body::from_stream(chunks).into_bytes().await?, "hello");
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_stream<S, E>(stream: S) -> Self
    where
        S: Stream<Item = Result<Bytes, E>> + Send + 'static,
        E: Into<BoxError> + 'static,
    {
        Self::new(StreamBody::new(stream.map_ok(Frame::data)))
    }

    /// Any body of [`Bytes`].
    ///
    /// A `Body` or poem's box is kept as it is. Any other body, `Sync` or not,
    /// is boxed once more inside a `Mutex` that each frame reaches through
    /// `get_mut`, never by locking: that is how a body that is not `Sync` (a
    /// stream's) still lets a `&Request` holding it cross an `.await` in a
    /// `Send` future.
    ///
    /// ```
    /// use nest_rs_http::{Body, Bytes};
    ///
    /// let full = http_body_util::Full::new(Bytes::from_static(b"{}"));
    /// assert!(!Body::new(full).is_empty());
    /// ```
    pub fn new<B>(body: B) -> Self
    where
        B: http_body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        let mut slot = Some(body);
        let any = &mut slot as &mut dyn Any;
        if let Some(body) = any.downcast_mut::<Option<Self>>().and_then(Option::take) {
            return body;
        }
        if let Some(boxed) = any
            .downcast_mut::<Option<BoxBody<Bytes, io::Error>>>()
            .and_then(Option::take)
        {
            return Self(boxed);
        }
        match slot {
            Some(body) => Self(BoxBody::new(Exclusive::new(body).map_err(into_io))),
            // Unreachable: `slot` is emptied only by a `take` that returned above.
            None => Self::empty(),
        }
    }

    /// Whether the body is known to hold no bytes.
    ///
    /// ```
    /// use nest_rs_http::Body;
    ///
    /// assert!(Body::from("").is_empty());
    /// assert!(!Body::from("x").is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        http_body::Body::size_hint(&self.0).exact() == Some(0)
    }

    /// Every byte of the body, read to its end.
    ///
    /// ```
    /// use nest_rs_http::Body;
    ///
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// assert_eq!(Body::from(vec![1, 2]).into_bytes().await?, vec![1u8, 2]);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn into_bytes(self) -> Result<Bytes, BodyError> {
        self.into_bytes_limit(usize::MAX).await
    }

    /// Every byte of the body, refused with [`BodyError::TooLarge`] as soon as
    /// it would exceed `limit`: a declared size past it reads nothing, and a
    /// frame crossing it is the last one polled.
    ///
    /// ```
    /// use nest_rs_http::{Body, BodyError};
    ///
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// let refused = Body::from("too long").into_bytes_limit(3).await;
    /// assert!(matches!(refused, Err(BodyError::TooLarge { limit: 3 })));
    /// # Ok(())
    /// # }
    /// ```
    pub async fn into_bytes_limit(self, limit: usize) -> Result<Bytes, BodyError> {
        let mut body = self.0;
        let declared = http_body::Body::size_hint(&body).lower();
        if declared > u64::try_from(limit).unwrap_or(u64::MAX) {
            return Err(BodyError::TooLarge { limit });
        }
        let mut read = Collected::default();
        while let Some(frame) = body.frame().await {
            let Ok(data) = frame.map_err(BodyError::Io)?.into_data() else {
                continue;
            };
            if read.len().saturating_add(data.len()) > limit {
                return Err(BodyError::TooLarge { limit });
            }
            read.push(data);
        }
        Ok(read.into_bytes())
    }

    /// The body read to its end as UTF-8 text.
    ///
    /// ```
    /// use nest_rs_http::{Body, BodyError};
    ///
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// let refused = Body::from(vec![0xff]).into_string().await;
    /// assert!(matches!(refused, Err(BodyError::NotUtf8)));
    /// # Ok(())
    /// # }
    /// ```
    pub async fn into_string(self) -> Result<String, BodyError> {
        let bytes = self.into_bytes().await?;
        String::from_utf8(Vec::from(bytes))
            .ok()
            .ok_or(BodyError::NotUtf8)
    }

    /// The body's data frames as a stream; trailers are dropped.
    ///
    /// ```
    /// use futures_util::TryStreamExt;
    /// use nest_rs_http::Body;
    ///
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// let chunks: Vec<_> = Body::from("ada").into_stream().try_collect().await?;
    /// assert_eq!(chunks.concat(), b"ada");
    /// # Ok(())
    /// # }
    /// ```
    pub fn into_stream(self) -> impl Stream<Item = io::Result<Bytes>> + Send + 'static {
        BodyDataStream::new(self.0)
    }

    /// A body around `boxed`: poem wraps the same box, which the bridge moves.
    pub(crate) fn from_boxed(boxed: BoxBody<Bytes, io::Error>) -> Self {
        Self(boxed)
    }

    /// The box, for poem, which wraps the same type.
    pub(crate) fn into_boxed(self) -> BoxBody<Bytes, io::Error> {
        self.0
    }
}

impl http_body::Body for Body {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        Pin::new(&mut self.0).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.0.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.0.size_hint()
    }
}

impl Default for Body {
    fn default() -> Self {
        Self::empty()
    }
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Body")
            .field("size_hint", &http_body::Body::size_hint(&self.0))
            .finish()
    }
}

impl From<Bytes> for Body {
    fn from(bytes: Bytes) -> Self {
        Self::from_bytes(bytes)
    }
}

impl From<String> for Body {
    fn from(text: String) -> Self {
        Self::from_string(text)
    }
}

impl From<&'static str> for Body {
    fn from(text: &'static str) -> Self {
        Self::from_bytes(Bytes::from_static(text.as_bytes()))
    }
}

impl From<Vec<u8>> for Body {
    fn from(bytes: Vec<u8>) -> Self {
        Self::from_bytes(Bytes::from(bytes))
    }
}

impl From<&'static [u8]> for Body {
    fn from(bytes: &'static [u8]) -> Self {
        Self::from_bytes(Bytes::from_static(bytes))
    }
}

impl From<()> for Body {
    fn from((): ()) -> Self {
        Self::empty()
    }
}

fn never(never: Infallible) -> io::Error {
    match never {}
}

/// A body's error as the `io::Error` the box carries; one already an
/// `io::Error` keeps its kind.
fn into_io<E: Into<BoxError>>(error: E) -> io::Error {
    match error.into().downcast::<io::Error>() {
        Ok(io) => *io,
        Err(other) => io::Error::other(other),
    }
}

/// A `Send` body made `Sync` without a lock on the frame path: `poll_frame`
/// holds `&mut self`, so `Mutex::get_mut` reaches the body; the hints a `&self`
/// reads are kept from the last frame.
struct Exclusive<B> {
    body: Mutex<Pin<Box<B>>>,
    hint: SizeHint,
    ended: bool,
}

impl<B: http_body::Body> Exclusive<B> {
    fn new(body: B) -> Self {
        let hint = body.size_hint();
        let ended = body.is_end_stream();
        Self {
            body: Mutex::new(Box::pin(body)),
            hint,
            ended,
        }
    }
}

impl<B: http_body::Body + Send> http_body::Body for Exclusive<B> {
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<B::Data>, B::Error>>> {
        let this = self.get_mut();
        // The lock is never taken, so never poisoned.
        let body = this.body.get_mut().unwrap_or_else(PoisonError::into_inner);
        let polled = body.as_mut().poll_frame(cx);
        this.hint = body.size_hint();
        this.ended = body.is_end_stream();
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.ended
    }

    fn size_hint(&self) -> SizeHint {
        self.hint
    }
}

/// The data frames read so far: one frame is kept as is, several are copied
/// once into one buffer.
#[derive(Default)]
struct Collected {
    first: Option<Bytes>,
    rest: Option<BytesMut>,
}

impl Collected {
    fn len(&self) -> usize {
        match (&self.first, &self.rest) {
            (_, Some(rest)) => rest.len(),
            (Some(first), None) => first.len(),
            (None, None) => 0,
        }
    }

    fn push(&mut self, data: Bytes) {
        if data.is_empty() {
            return;
        }
        if let Some(rest) = &mut self.rest {
            rest.extend_from_slice(&data);
        } else if let Some(first) = self.first.take() {
            let mut rest = BytesMut::with_capacity(first.len() + data.len());
            rest.extend_from_slice(&first);
            rest.extend_from_slice(&data);
            self.rest = Some(rest);
        } else {
            self.first = Some(data);
        }
    }

    fn into_bytes(self) -> Bytes {
        match (self.first, self.rest) {
            (_, Some(rest)) => rest.freeze(),
            (Some(first), None) => first,
            (None, None) => Bytes::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures_util::StreamExt;

    use super::*;

    /// Frames served one per poll, counting how many were asked for.
    fn counted(frames: &[&'static [u8]], polled: Arc<AtomicUsize>) -> Body {
        let frames: Vec<_> = frames.iter().map(|f| Bytes::from_static(f)).collect();
        Body::from_stream(futures_util::stream::iter(frames).map(move |frame| {
            polled.fetch_add(1, Ordering::SeqCst);
            Ok::<_, io::Error>(frame)
        }))
    }

    #[tokio::test]
    async fn into_bytes_limit_fails_too_large_without_reading_past_the_next_frame() {
        let polled = Arc::new(AtomicUsize::new(0));
        let body = counted(&[b"abc", b"defg", b"never read"], polled.clone());

        let refused = body.into_bytes_limit(5).await;

        assert!(matches!(refused, Err(BodyError::TooLarge { limit: 5 })));
        assert_eq!(
            polled.load(Ordering::SeqCst),
            2,
            "the frame crossing the limit is the last one read",
        );
    }

    /// A body declaring more bytes than it serves, counting the frames asked
    /// for: only the declaration can refuse it.
    struct Declared {
        declared: u64,
        frame: Option<Bytes>,
        polled: Arc<AtomicUsize>,
    }

    impl http_body::Body for Declared {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
            self.polled.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(self.frame.take().map(|data| Ok(Frame::data(data))))
        }

        fn size_hint(&self) -> SizeHint {
            SizeHint::with_exact(self.declared)
        }
    }

    #[tokio::test]
    async fn a_declared_size_past_the_limit_reads_nothing() {
        let polled = Arc::new(AtomicUsize::new(0));
        let body = Body::new(Declared {
            declared: 10,
            frame: Some(Bytes::from_static(b"ab")),
            polled: polled.clone(),
        });

        let refused = body.into_bytes_limit(4).await;

        assert!(matches!(refused, Err(BodyError::TooLarge { limit: 4 })));
        assert_eq!(polled.load(Ordering::SeqCst), 0, "no frame was asked for");
    }

    #[tokio::test]
    async fn a_body_at_the_limit_is_read_whole() {
        let polled = Arc::new(AtomicUsize::new(0));
        let read = counted(&[b"ab", b"cd"], polled).into_bytes_limit(4).await;
        assert_eq!(read.unwrap(), "abcd");
    }

    #[tokio::test]
    async fn a_failing_stream_is_an_io_error_keeping_its_kind() {
        let failing = futures_util::stream::iter([Err::<Bytes, _>(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "peer reset",
        ))]);
        match Body::from_stream(failing).into_bytes().await {
            Err(BodyError::Io(io)) => assert_eq!(io.kind(), io::ErrorKind::ConnectionReset),
            other => panic!("expected an io error, got {other:?}"),
        }
    }

    /// A body that is `Send` but not `Sync`, as a stream's can be.
    struct NotSync {
        data: Option<Bytes>,
        _not_sync: std::marker::PhantomData<std::cell::Cell<()>>,
    }

    impl http_body::Body for NotSync {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
            Poll::Ready(self.data.take().map(|data| Ok(Frame::data(data))))
        }
    }

    fn assert_sync<T: Sync>() {}

    #[tokio::test]
    async fn a_body_that_is_not_sync_is_accepted_and_request_stays_sync() {
        assert_sync::<Body>();
        assert_sync::<crate::Request>();
        let body = Body::new(NotSync {
            data: Some(Bytes::from_static(b"frame")),
            _not_sync: std::marker::PhantomData,
        });
        assert_eq!(body.into_bytes().await.unwrap(), "frame");
    }
}

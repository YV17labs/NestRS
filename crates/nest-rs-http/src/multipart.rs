//! [`Multipart`]: a `multipart/form-data` body read part by part, each part
//! buffered or streamed as it arrives; the parser is multer's.
//!
//! The request body stays capped at [`HttpConfig.max_body_bytes`](crate::HttpConfig):
//! streaming bounds memory, not the request.
#![expect(
    clippy::disallowed_types,
    reason = "multer is the multipart parser this file wraps, and the one that names it"
)]

use std::borrow::Cow;
use std::fmt;
use std::io::{self, Result as IoResult};
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::{Stream, TryStreamExt};
use tokio::io::AsyncRead;
use tokio_util::io::{ReaderStream, StreamReader};

use crate::error::{BodyError, HttpError, MultipartError, Result};
use crate::media_type::{MediaType, Unparsed, media_type};
use crate::request::{FromRequest, Request, RequestBody};

/// A `multipart/form-data` body, read one part at a time as it arrives.
///
/// A request without that `Content-Type` answers `415`; a body that does not
/// parse as parts answers `400`, saying nothing of what it held.
///
/// ```
/// use nest_rs_http::{FromRequest, HeaderValue, Multipart, Request, header};
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
///
/// let form = "--x\r\ncontent-disposition: form-data; name=\"title\"\r\n\r\nHello\r\n--x--\r\n";
/// let (request, mut body) = Request::builder()
///     .header(header::CONTENT_TYPE, HeaderValue::from_static("multipart/form-data; boundary=x"))
///     .body(form)
///     .split();
/// let mut parts = Multipart::from_request(&request, &mut body).await?;
/// let title = parts.next_field().await?.expect("one part");
/// assert_eq!(title.name(), Some("title"));
/// assert_eq!(title.text().await?, "Hello");
/// assert!(parts.next_field().await?.is_none());
/// # Ok(())
/// # }
/// ```
pub struct Multipart {
    parts: multer::Multipart<'static>,
}

impl Multipart {
    /// The next part, or `None` past the last; a part not read whole is
    /// skipped.
    pub async fn next_field(&mut self) -> Result<Option<Field>> {
        self.parts
            .next_field()
            .await
            .map(|part| part.map(|part| Field { part }))
            .map_err(malformed)
    }
}

impl fmt::Debug for Multipart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Multipart").finish_non_exhaustive()
    }
}

impl<'a> FromRequest<'a> for Multipart {
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        let Some(media) = req
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(media_type)
            .filter(|media| media.is_exactly("multipart", "form-data"))
        else {
            return Err(MultipartError::MediaType.into());
        };
        let boundary = boundary_of(&media).map_err(MultipartError::from)?;
        Ok(Self {
            parts: multer::Multipart::new(body.take()?.into_stream(), boundary.into_owned()),
        })
    }
}

/// Why a multipart type names no boundary its body can be read by, said by
/// kind, never by the type the client wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BoundaryRefused {
    /// Its parameters are not the list RFC 9110 §5.6.6 writes.
    Unparsed,
    /// It names no boundary.
    Missing,
    /// It names more than one, which two readers could each take otherwise
    /// (RFC 6838 §4.3).
    Repeated,
    /// Its boundary is not 1 to 70 of the characters RFC 2046 §5.1.1 allows,
    /// the last no space.
    Invalid,
}

impl BoundaryRefused {
    /// What the refusal says after the body's own sentence.
    fn said(self) -> &'static str {
        match self {
            Self::Unparsed => "its type does not parse",
            Self::Missing => "its type names no boundary",
            Self::Repeated => "its type names more than one boundary",
            Self::Invalid => "its boundary is not one RFC 2046 admits",
        }
    }
}

impl From<BoundaryRefused> for MultipartError {
    fn from(refused: BoundaryRefused) -> Self {
        Self::Malformed(refused.said().into())
    }
}

impl fmt::Display for BoundaryRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&MultipartError::from(*self), f)
    }
}

impl std::error::Error for BoundaryRefused {}

/// The boundary `content_type` names, read off the edge's own parse of the
/// type, so every multipart body the edge reads, `/graphql`'s too, admits what
/// RFC 9110 §5.6.6 does and refuses the same.
pub fn boundary(content_type: &str) -> std::result::Result<Cow<'_, str>, BoundaryRefused> {
    boundary_of(&media_type(content_type).ok_or(BoundaryRefused::Unparsed)?)
}

/// The one `boundary` parameter of `media`, as RFC 2046 §5.1.1 writes one.
fn boundary_of<'a>(media: &MediaType<'a>) -> std::result::Result<Cow<'a, str>, BoundaryRefused> {
    let mut named = None;
    for parameter in media.parameters() {
        let parameter = parameter.map_err(|Unparsed| BoundaryRefused::Unparsed)?;
        if parameter.name.eq_ignore_ascii_case("boundary")
            && named.replace(parameter.value).is_some()
        {
            return Err(BoundaryRefused::Repeated);
        }
    }
    let boundary = named.ok_or(BoundaryRefused::Missing)?;
    let admitted = (1..=70).contains(&boundary.len())
        && !boundary.ends_with(' ')
        && boundary
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"'()+_,-./:=? ".contains(&byte));
    admitted.then_some(boundary).ok_or(BoundaryRefused::Invalid)
}

/// What the parser refused, said by kind: its own sentence names the field, the
/// header or the bytes the client sent. A body that could not be read is no
/// refusal of the parser's, and each caller hands it on first ([`body_failed`]).
fn by_kind(refused: multer::Error) -> MultipartError {
    use multer::Error as Refused;
    let said = match refused {
        Refused::UnknownField { .. } => "a part the form does not take",
        Refused::IncompleteFieldData { .. } => "a part's data ends before its boundary",
        Refused::IncompleteHeaders => "a part's headers end before their blank line",
        Refused::ReadHeaderFailed(_) => "a part's headers do not parse",
        Refused::DecodeHeaderName { .. } => "a part's header name does not decode",
        Refused::DecodeHeaderValue { .. } => "a part's header value does not decode",
        Refused::IncompleteStream => "the body ends before its closing boundary",
        Refused::FieldSizeExceeded { .. } => "a part is larger than its limit",
        Refused::StreamSizeExceeded { .. } => "the body is larger than its limit",
        _ => "its parts do not parse",
    };
    MultipartError::Malformed(said.into())
}

/// The body's own read failure, as the body failed: its stream yields
/// `io::Error`s, which multer boxes.
fn body_failed(cause: Box<dyn std::error::Error + Send + Sync>) -> io::Error {
    cause
        .downcast::<io::Error>()
        .map_or_else(io::Error::other, |failed| *failed)
}

/// What the parser refused, as the extractor answers it: the body's own read
/// failure as the edge answers one, any other said by kind.
fn malformed(refused: multer::Error) -> HttpError {
    match refused {
        multer::Error::StreamReadFailed(cause) => BodyError::Io(body_failed(cause)).into(),
        refused => by_kind(refused).into(),
    }
}

/// What the parser refused, as a part's stream fails with it: the body's own
/// read failure as the body failed, any other said by kind.
fn unreadable(refused: multer::Error) -> io::Error {
    match refused {
        multer::Error::StreamReadFailed(cause) => body_failed(cause),
        refused => io::Error::new(io::ErrorKind::InvalidData, by_kind(refused)),
    }
}

/// One part of a [`Multipart`] body: its name, its file name and media type
/// when the client sent them, and its bytes.
pub struct Field {
    part: multer::Field<'static>,
}

impl Field {
    /// The form field's name.
    pub fn name(&self) -> Option<&str> {
        self.part.name()
    }

    /// The file name a client sent with the part, as it sent it: never a path
    /// to write to as is.
    pub fn file_name(&self) -> Option<&str> {
        self.part.file_name()
    }

    /// The part's media type, its parameters dropped.
    pub fn content_type(&self) -> Option<&str> {
        self.part.content_type().map(|media| media.essence_str())
    }

    /// The whole part, buffered.
    pub async fn bytes(self) -> Result<Bytes> {
        self.part.bytes().await.map_err(malformed)
    }

    /// The whole part, buffered, as UTF-8 text; a part that is not answers
    /// `400`.
    pub async fn text(self) -> Result<String> {
        let bytes = self.bytes().await?;
        String::from_utf8(bytes.into()).map_err(|refused| {
            let said = format!(
                "a part's text is not UTF-8 past its byte {}",
                refused.utf8_error().valid_up_to()
            );
            MultipartError::Malformed(said.into()).into()
        })
    }

    /// The part's bytes as they arrive, for a sink that consumes a stream — an
    /// object store's streamed upload — rather than a buffered part.
    ///
    /// A part the parser refuses fails the stream with an
    /// [`InvalidData`](io::ErrorKind::InvalidData) error saying it by kind, as
    /// [`bytes`](Self::bytes) would; a body that cannot be read fails it with
    /// the body's own error.
    pub fn into_byte_stream(self) -> PartStream {
        let read = StreamReader::new(self.part.map_err(unreadable));
        PartStream(ReaderStream::new(Box::pin(read)))
    }
}

impl fmt::Debug for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Field").finish_non_exhaustive()
    }
}

/// One multipart part's bytes, as they arrive. Yielded chunks are whatever the
/// transport read — never a whole part.
pub struct PartStream(ReaderStream<Pin<Box<dyn AsyncRead + Send>>>);

impl Stream for PartStream {
    type Item = IoResult<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.get_mut().0).poll_next(cx)
    }
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use http::{HeaderValue, StatusCode, header};

    use super::*;

    /// One `file` part holding `payload`, as a client sends it.
    fn upload(payload: &[u8]) -> (Request, RequestBody) {
        let mut form =
            b"--x\r\ncontent-disposition: form-data; name=\"file\"; filename=\"a\"\r\n\r\n"
                .to_vec();
        form.extend_from_slice(payload);
        form.extend_from_slice(b"\r\n--x--\r\n");
        Request::builder()
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_static("multipart/form-data; boundary=x"),
            )
            .body(form)
            .split()
    }

    /// The `file` part read as a stream: how many chunks, how many bytes.
    async fn streamed(payload: &[u8]) -> (usize, usize) {
        let (request, mut body) = upload(payload);
        let mut form = Multipart::from_request(&request, &mut body)
            .await
            .expect("a form");
        let field = form.next_field().await.expect("parts").expect("one part");
        let mut stream = field.into_byte_stream();
        let (mut chunks, mut bytes) = (0, 0);
        while let Some(chunk) = stream.next().await {
            chunks += 1;
            bytes += chunk.expect("a chunk").len();
        }
        (chunks, bytes)
    }

    #[tokio::test]
    async fn a_part_streams_its_bytes_without_being_buffered_whole() {
        let payload = vec![b'a'; 512 * 1024];
        let (chunks, bytes) = streamed(&payload).await;
        assert_eq!(bytes, payload.len(), "every byte arrives");
        assert!(chunks > 1, "and they arrive in pieces, not one buffer");
    }

    #[tokio::test]
    async fn an_empty_part_yields_no_bytes_rather_than_failing() {
        assert_eq!(streamed(&[]).await.1, 0);
    }

    const FORM: &str = "--x\r\n\
        content-disposition: form-data; name=\"title\"\r\n\r\n\
        Hello\r\n\
        --x\r\n\
        content-disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\
        content-type: text/plain\r\n\r\n\
        0123456789\r\n\
        --x--\r\n";

    async fn parts(
        content_type: &'static str,
        body: &'static str,
    ) -> Result<Multipart, crate::HttpError> {
        let (request, mut body) = crate::Request::builder()
            .header(header::CONTENT_TYPE, HeaderValue::from_static(content_type))
            .body(body)
            .split();
        Multipart::from_request(&request, &mut body).await
    }

    #[tokio::test]
    async fn each_part_reads_its_name_file_name_type_and_bytes() {
        let mut form = parts("multipart/form-data; boundary=x", FORM)
            .await
            .unwrap();

        let title = form.next_field().await.unwrap().unwrap();
        assert_eq!((title.name(), title.file_name()), (Some("title"), None));
        assert_eq!(title.text().await.unwrap(), "Hello");

        let file = form.next_field().await.unwrap().unwrap();
        assert_eq!(file.name(), Some("file"));
        assert_eq!(file.file_name(), Some("a.txt"));
        assert_eq!(file.content_type(), Some("text/plain"));
        let mut stream = file.into_byte_stream();
        let mut read = Vec::new();
        while let Some(chunk) = stream.next().await {
            read.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(read, b"0123456789");

        assert!(form.next_field().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_body_that_is_not_multipart_by_its_type_is_415_and_its_type_is_never_quoted() {
        let refused = parts("text/plain; boundary=sk_live_51", FORM)
            .await
            .unwrap_err();
        assert_eq!(refused.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let body = refused
            .into_response()
            .into_body()
            .into_string()
            .await
            .unwrap();
        assert!(body.contains("multipart/form-data"), "{body}");
        assert!(!body.contains("sk_live"), "{body}");
    }

    #[tokio::test]
    async fn a_body_that_does_not_parse_as_parts_is_400_saying_nothing_of_it() {
        let mut form = parts(
            "multipart/form-data; boundary=x",
            "--x\r\ncontent-disposition: form-data; name=\"sk_live_51\"\r\n\r\nunterminated",
        )
        .await
        .unwrap();
        let refused = match form.next_field().await {
            Ok(Some(field)) => field.bytes().await.unwrap_err(),
            Ok(None) => panic!("a part was sent"),
            Err(refused) => refused,
        };
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        let body = refused
            .into_response()
            .into_body()
            .into_string()
            .await
            .unwrap();
        assert!(body.contains("the multipart body is malformed"), "{body}");
        assert!(!body.contains("sk_live"), "{body}");
    }

    /// What the parser refused is kept for the operator's line by kind: no
    /// reader of the failure — its line, a `{:?}`, a `main` — reads a name, a
    /// header or a byte the client sent.
    #[tokio::test]
    async fn no_reader_of_a_refused_body_reads_what_the_client_sent() {
        let mut form = parts(
            "multipart/form-data; boundary=x",
            "--x\r\ncontent-disposition: form-data; name=\"sk_live_51\"\r\n\r\nunterminated",
        )
        .await
        .unwrap();
        let unterminated = match form.next_field().await {
            Ok(Some(field)) => field.bytes().await.unwrap_err(),
            Ok(None) => panic!("a part was sent"),
            Err(refused) => refused,
        };
        let printed = nest_rs_testing::printed(&unterminated);
        assert!(!printed.contains("sk_live"), "{printed}");
        assert!(
            printed.contains("the multipart body is malformed"),
            "{printed}"
        );

        let mut form = parts(
            "multipart/form-data; boundary=x",
            "--x\r\ncontent-disposition: form-data; name=\"sk_live_51\"\r\n\r\nunterminated",
        )
        .await
        .unwrap();
        let field = form.next_field().await.expect("parts").expect("one part");
        let cut_short = first_failure(field.into_byte_stream()).await;
        let printed = nest_rs_testing::printed(&cut_short);
        assert!(!printed.contains("sk_live"), "{printed}");
        assert!(
            printed.contains(
                "the multipart body is malformed: a part's data ends before its boundary"
            ),
            "{printed}"
        );
        assert_eq!(cut_short.kind(), io::ErrorKind::InvalidData);

        let (request, mut body) = upload(b"sk_live_\xff");
        let mut form = Multipart::from_request(&request, &mut body)
            .await
            .expect("a form");
        let field = form.next_field().await.expect("parts").expect("one part");
        let not_text = field.text().await.expect_err("no UTF-8");
        assert_eq!(not_text.status(), StatusCode::BAD_REQUEST);
        let printed = nest_rs_testing::printed(&not_text);
        let bytes = format!("{:?}", &b"sk_live_"[..]);
        let bytes = bytes.trim_start_matches('[').trim_end_matches(']');
        assert!(!printed.contains(bytes), "{printed}");
        assert!(
            printed.contains("a part's text is not UTF-8 past its byte 8"),
            "{printed}"
        );
    }

    /// The first error `stream` yields, past whatever bytes it yields first.
    async fn first_failure(mut stream: PartStream) -> io::Error {
        while let Some(chunk) = stream.next().await {
            if let Err(failed) = chunk {
                return failed;
            }
        }
        panic!("the stream ended without failing")
    }

    /// A form whose body arrives as the sender feeds it, so a test fails it
    /// past what the parser has read.
    async fn fed() -> (
        tokio::sync::mpsc::UnboundedSender<IoResult<Bytes>>,
        Multipart,
    ) {
        let (send, sent) = tokio::sync::mpsc::unbounded_channel::<IoResult<Bytes>>();
        let sent = futures_util::stream::unfold(sent, |mut sent| async move {
            sent.recv().await.map(|chunk| (chunk, sent))
        });
        let (request, mut body) = Request::builder()
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_static("multipart/form-data; boundary=x"),
            )
            .body(crate::body::Body::from_stream(sent))
            .split();
        let form = Multipart::from_request(&request, &mut body)
            .await
            .expect("a form");
        (send, form)
    }

    const PART_HEAD: &[u8] = b"--x\r\ncontent-disposition: form-data; name=\"file\"\r\n\r\nabc";

    fn reset() -> io::Error {
        io::Error::new(io::ErrorKind::ConnectionReset, "zzqx-peer reset")
    }

    #[tokio::test]
    async fn a_part_streamed_from_a_body_that_fails_fails_as_the_body_did() {
        let (send, mut form) = fed().await;
        send.send(Ok(Bytes::from_static(PART_HEAD)))
            .expect("the body is read");
        let field = form.next_field().await.expect("parts").expect("one part");
        send.send(Err(reset())).expect("the body is read");
        let failed = first_failure(field.into_byte_stream()).await;
        assert_eq!(failed.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(nest_rs_core::error_message(&failed), "zzqx-peer reset");
    }

    /// The status and body `refused` answers.
    async fn answered(refused: HttpError) -> (StatusCode, String) {
        let response = refused.into_response();
        let status = response.status();
        let body = response.into_body().into_string().await.unwrap();
        (status, body)
    }

    /// A body that fails under the extractor is the edge's own refusal of it,
    /// in its words: the transport's sentence stays in the chain, never in the
    /// detail.
    #[tokio::test]
    async fn a_body_that_fails_under_the_extractor_answers_as_the_edge_answers_one() {
        let expected = answered(BodyError::Io(reset()).into()).await;
        assert_eq!(expected.0, StatusCode::BAD_REQUEST);
        assert!(!expected.1.contains("zzqx"), "{}", expected.1);

        let (send, mut form) = fed().await;
        send.send(Ok(Bytes::from_static(b"--x\r\ncontent-disposition: form-")))
            .expect("the body is read");
        send.send(Err(reset())).expect("the body is read");
        let under_next_field = form.next_field().await.expect_err("the body failed");

        let (send, mut form) = fed().await;
        send.send(Ok(Bytes::from_static(PART_HEAD)))
            .expect("the body is read");
        let field = form.next_field().await.expect("parts").expect("one part");
        send.send(Err(reset())).expect("the body is read");
        let under_bytes = field.bytes().await.expect_err("the body failed");

        for refused in [under_next_field, under_bytes] {
            let Some(BodyError::Io(failed)) = refused.downcast_ref::<BodyError>() else {
                panic!("not the edge's refusal: {refused:?}");
            };
            assert_eq!(failed.kind(), io::ErrorKind::ConnectionReset);
            assert_eq!(answered(refused).await, expected);
        }
    }

    #[tokio::test]
    async fn a_type_that_does_not_parse_is_400_said_of_the_type_never_of_a_part() {
        let refused = parts("multipart/form-data; boundary=\"x", FORM)
            .await
            .unwrap_err();
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        let body = refused
            .into_response()
            .into_body()
            .into_string()
            .await
            .unwrap();
        assert!(
            body.contains("the multipart body is malformed: its type does not parse"),
            "{body}"
        );
    }

    /// RFC 9110 §5.6.6 admits whitespace around each `;`, an empty parameter
    /// and a name in any case, and §5.6.4 a quoted-pair; RFC 2046 §5.1.1 a
    /// boundary of up to 70 of its characters, a space among them.
    #[tokio::test]
    async fn every_type_the_rfcs_admit_reads_its_boundary() {
        const SPACED: &str = "--a b\r\ncontent-disposition: form-data; name=\"title\"\r\n\r\n\
            Hello\r\n--a b--\r\n";
        const LONGEST: &str = "--0123456789012345678901234567890123456789012345678901234567890123456789\r\n\
            content-disposition: form-data; name=\"title\"\r\n\r\nHello\r\n\
            --0123456789012345678901234567890123456789012345678901234567890123456789--\r\n";
        for (content_type, body) in [
            ("multipart/form-data ; boundary=x", FORM),
            ("multipart/form-data;boundary=x", FORM),
            ("multipart/form-data;\tboundary=x", FORM),
            ("multipart/form-data\t;\tboundary=x\t", FORM),
            ("multipart/form-data; boundary=x;;", FORM),
            ("multipart/form-data;; boundary=x", FORM),
            ("multipart/form-data; boundary=x ;", FORM),
            ("multipart/form-data; BOUNDARY=x", FORM),
            ("Multipart/Form-Data; boundary=\"x\"", FORM),
            ("multipart/form-data; charset=utf-8; boundary=x", FORM),
            ("multipart/form-data; boundary=\"\\x\"", FORM),
            ("multipart/form-data; boundary=\"a b\"", SPACED),
            (
                "multipart/form-data; boundary=0123456789012345678901234567890123456789012345678901234567890123456789",
                LONGEST,
            ),
        ] {
            let mut form = parts(content_type, body)
                .await
                .unwrap_or_else(|refused| panic!("{content_type}: {refused:?}"));
            let title = form.next_field().await.unwrap().unwrap();
            assert_eq!(title.name(), Some("title"), "{content_type}");
        }
    }

    /// A type a parameter list does not parse, a boundary RFC 2046 §5.1.1 does
    /// not admit, or one named twice, which two readers could each take
    /// otherwise (RFC 6838 §4.3), is refused saying which, never the value.
    #[tokio::test]
    async fn a_boundary_the_rfcs_do_not_admit_is_400_said_by_kind() {
        for (content_type, said) in [
            ("multipart/form-data; boundary", "its type does not parse"),
            ("multipart/form-data; =x", "its type does not parse"),
            (
                "multipart/form-data; boundary=x y",
                "its type does not parse",
            ),
            (
                "multipart/form-data; boundary=\"x\" y",
                "its type does not parse",
            ),
            (
                "multipart/form-data; boundary=\"x\\",
                "its type does not parse",
            ),
            (
                "multipart/form-data; boundary=x, charset=utf-8",
                "its type does not parse",
            ),
            (
                "multipart/form-data; charset=utf-8",
                "its type names no boundary",
            ),
            ("multipart/form-data", "its type names no boundary"),
            (
                "multipart/form-data; boundary=x; Boundary=y",
                "its type names more than one boundary",
            ),
            (
                "multipart/form-data; boundary=\"\"",
                "its boundary is not one RFC 2046 admits",
            ),
            (
                "multipart/form-data; boundary=\"x \"",
                "its boundary is not one RFC 2046 admits",
            ),
            (
                "multipart/form-data; boundary=sk_live!",
                "its boundary is not one RFC 2046 admits",
            ),
            (
                "multipart/form-data; boundary=01234567890123456789012345678901234567890123456789012345678901234567890",
                "its boundary is not one RFC 2046 admits",
            ),
        ] {
            let refused = parts(content_type, FORM)
                .await
                .map(|_| ())
                .expect_err(content_type);
            assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "{content_type}");
            let body = refused
                .into_response()
                .into_body()
                .into_string()
                .await
                .unwrap();
            assert!(
                body.contains(&format!("the multipart body is malformed: {said}")),
                "{content_type}: {body}"
            );
            assert!(!body.contains("sk_live"), "{body}");
        }
    }
}

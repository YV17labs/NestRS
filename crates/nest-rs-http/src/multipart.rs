//! [`Multipart`]: a `multipart/form-data` body read part by part, each part
//! buffered or streamed as it arrives.
//!
//! The request body stays capped at [`HttpConfig.max_body_bytes`](crate::HttpConfig):
//! streaming bounds memory, not the request.

use std::fmt;
use std::io::Result as IoResult;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::Stream;
use tokio::io::AsyncRead;
use tokio_util::io::ReaderStream;

use crate::error::{MultipartError, Result};
use crate::poem_adapter::{FieldReader, MultipartReader, PoemAdapter};
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
    reader: MultipartReader,
}

impl Multipart {
    /// The next part, or `None` past the last; a part not read whole is
    /// skipped.
    pub async fn next_field(&mut self) -> Result<Option<Field>> {
        Ok(self
            .reader
            .next_field()
            .await?
            .map(|reader| Field { reader }))
    }
}

impl fmt::Debug for Multipart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Multipart").finish_non_exhaustive()
    }
}

impl<'a> FromRequest<'a> for Multipart {
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        let Some(content_type) = req
            .headers()
            .get(http::header::CONTENT_TYPE)
            .filter(|value| {
                value
                    .to_str()
                    .ok()
                    .and_then(crate::request::essence)
                    .is_some_and(|essence| essence.is_exactly("multipart", "form-data"))
            })
        else {
            return Err(MultipartError::MediaType.into());
        };
        let reader = PoemAdapter::multipart(content_type.clone(), body.take()?).await?;
        Ok(Self { reader })
    }
}

/// One part of a [`Multipart`] body: its name, its file name and media type
/// when the client sent them, and its bytes.
pub struct Field {
    reader: FieldReader,
}

impl Field {
    /// The form field's name.
    pub fn name(&self) -> Option<&str> {
        self.reader.name()
    }

    /// The file name a client sent with the part, as it sent it: never a path
    /// to write to as is.
    pub fn file_name(&self) -> Option<&str> {
        self.reader.file_name()
    }

    /// The part's media type, its parameters dropped.
    pub fn content_type(&self) -> Option<&str> {
        self.reader.content_type()
    }

    /// The whole part, buffered.
    pub async fn bytes(self) -> Result<Bytes> {
        self.reader.bytes().await
    }

    /// The whole part, buffered, as UTF-8 text; a part that is not answers
    /// `400`.
    pub async fn text(self) -> Result<String> {
        let bytes = self.reader.bytes().await?;
        String::from_utf8(bytes.into())
            .map_err(|refused| MultipartError::Malformed(Box::new(refused)).into())
    }

    /// The part's bytes as they arrive, for a sink that consumes a stream — an
    /// object store's streamed upload — rather than a buffered part.
    pub fn into_byte_stream(self) -> PartStream {
        PartStream(ReaderStream::new(Box::pin(self.reader.into_async_read())))
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

/// Read a poem multipart part without buffering it.
pub trait PartExt {
    /// The part's bytes as they arrive, for a sink that consumes a stream — an
    /// object store's streamed upload — rather than a buffered part.
    ///
    /// ```
    /// # use nest_rs_http::{Opaque, PartExt};
    /// # use nest_rs_http::poem::test::{TestClient, TestForm};
    /// # use nest_rs_http::poem::web::Multipart;
    /// # use nest_rs_http::poem::{Result, handler};
    /// use futures_util::TryStreamExt;
    /// # #[handler]
    /// # async fn upload(mut form: Multipart) -> Result<String> {
    /// # let Some(part) = form.next_field().await? else { return Ok(String::new()) };
    ///
    /// let mut stream = part.into_byte_stream();
    /// let mut received = 0;
    /// while let Some(chunk) = stream.try_next().await.opaque()? {
    ///     received += chunk.len();
    /// }
    /// # Ok(received.to_string())
    /// # }
    /// # #[nest_rs_core::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// # let audio = vec![7u8; 512 * 1024];
    /// # let form = TestForm::new().bytes("file", audio.clone());
    /// # let reply = TestClient::new(upload).post("/").multipart(form).send().await;
    /// # let received = reply.0.into_body().into_string().await?;
    ///
    /// assert_eq!(received, audio.len().to_string());
    /// # Ok(())
    /// # }
    /// ```
    fn into_byte_stream(self) -> PartStream;
}

impl PartExt for poem::web::Field {
    fn into_byte_stream(self) -> PartStream {
        PartStream(ReaderStream::new(Box::pin(self.into_async_read())))
    }
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use http::{HeaderValue, StatusCode, header};
    use poem::test::TestClient;
    use poem::{Result, handler};

    use super::*;

    #[handler]
    async fn upload(mut form: poem::web::Multipart) -> Result<String> {
        let mut chunks = 0usize;
        let mut body = Vec::new();
        while let Some(field) = form.next_field().await? {
            if field.name() != Some("file") {
                continue;
            }
            let mut stream = PartExt::into_byte_stream(field);
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(poem::error::InternalServerError)?;
                chunks += 1;
                body.extend_from_slice(&chunk);
            }
        }
        Ok(format!("{chunks}:{}", body.len()))
    }

    #[tokio::test]
    async fn a_part_streams_its_bytes_without_being_buffered_whole() {
        // Big enough that poem hands it over in several reads.
        let payload = vec![b'a'; 512 * 1024];
        let form = poem::test::TestForm::new().bytes("file", payload.clone());
        let resp = TestClient::new(upload)
            .post("/")
            .multipart(form)
            .send()
            .await;
        resp.assert_status_is_ok();
        let seen = resp.0.into_body().into_string().await.expect("body");
        let (chunks, bytes) = seen.split_once(':').expect("chunks:bytes");
        assert_eq!(
            bytes.parse::<usize>().expect("byte count"),
            payload.len(),
            "every byte arrives",
        );
        assert!(
            chunks.parse::<usize>().expect("chunk count") > 1,
            "and they arrive in pieces, not one buffer: {seen}",
        );
    }

    #[tokio::test]
    async fn an_empty_part_yields_no_bytes_rather_than_failing() {
        let form = poem::test::TestForm::new().bytes("file", Vec::new());
        let resp = TestClient::new(upload)
            .post("/")
            .multipart(form)
            .send()
            .await;
        resp.assert_status_is_ok();
        let seen = resp.0.into_body().into_string().await.expect("body");
        assert!(seen.ends_with(":0"), "no bytes: {seen}");
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
}
